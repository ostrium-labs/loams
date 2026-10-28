//! The generic key commands (plan KV2 Task 3): `DEL`, `UNLINK`, `EXISTS`,
//! `TOUCH`, `TYPE`, the `EXPIRE` family, `TTL`, `PTTL`, `EXPIRETIME`,
//! `PEXPIRETIME`, `PERSIST`, `DBSIZE`. Replies and error texts follow
//! Valkey's documented behaviour.

use std::sync::Arc;

use bytes::Bytes;

use super::{Spec, int_arg, txn, upper};
use crate::frame::Value;
use crate::layout::{self, Meta};
use crate::server::{Conn, Shared};
use crate::types::{self, Kv, delete_key, put_meta, read_live};

/// The meta keys one `DBSIZE` transaction reads at most.
const DBSIZE_PAGE: usize = 1024;

pub(super) async fn run(
    shared: &Arc<Shared>,
    conn: &mut Conn,
    spec: &Spec,
    args: &[Bytes],
) -> Value {
    let db = conn.db;
    let result = match spec.name {
        "del" | "unlink" => del(shared, db, args).await,
        "exists" | "touch" => exists(shared, db, args).await,
        "type" => key_type(shared, db, args[1].clone()).await,
        "expire" => expire(shared, db, args, 1000, false, spec.name).await,
        "pexpire" => expire(shared, db, args, 1, false, spec.name).await,
        "expireat" => expire(shared, db, args, 1000, true, spec.name).await,
        "pexpireat" => expire(shared, db, args, 1, true, spec.name).await,
        "ttl" => ttl(shared, db, args[1].clone(), Ttl::Seconds).await,
        "pttl" => ttl(shared, db, args[1].clone(), Ttl::Millis).await,
        "expiretime" => ttl(shared, db, args[1].clone(), Ttl::AtSeconds).await,
        "pexpiretime" => ttl(shared, db, args[1].clone(), Ttl::AtMillis).await,
        "persist" => persist(shared, db, args[1].clone()).await,
        "dbsize" => dbsize(shared, db).await,
        other => Err(Value::err(format!("ERR '{other}' has no handler"))),
    };
    result.unwrap_or_else(|e| e)
}

async fn del(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    let keys: Vec<Bytes> = args[1..].to_vec();
    let n = txn(shared, "resp.del", move |t| {
        let keys = keys.clone();
        Box::pin(async move {
            let now = types::now_ms();
            let mut n = 0i64;
            for k in &keys {
                if delete_key(t, db, k, now).await? {
                    n += 1;
                }
            }
            Ok(n)
        })
    })
    .await?;
    Ok(Value::Integer(n))
}

/// `EXISTS` and `TOUCH`: how many of the keys are live (a key named twice
/// counts twice).
async fn exists(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    let keys: Vec<Bytes> = args[1..].to_vec();
    let n = txn(shared, "resp.exists", move |t| {
        let keys = keys.clone();
        Box::pin(async move {
            let now = types::now_ms();
            let mut n = 0i64;
            for k in &keys {
                if read_live(t, db, k, now).await?.live.is_some() {
                    n += 1;
                }
            }
            Ok(n)
        })
    })
    .await?;
    Ok(Value::Integer(n))
}

async fn key_type(shared: &Shared, db: u8, key: Bytes) -> Result<Value, Value> {
    let kind = txn(shared, "resp.type", move |t| {
        let key = key.clone();
        Box::pin(async move {
            let found = read_live(t, db, &key, types::now_ms()).await?;
            Ok(found.live.map(|m| m.kind))
        })
    })
    .await?;
    Ok(Value::Simple(
        kind.map_or("none", layout::Kind::name).to_owned(),
    ))
}

/// `EXPIRE`'s condition flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Cond {
    nx: bool,
    xx: bool,
    gt: bool,
    lt: bool,
}

impl Cond {
    /// Parses `NX`, `XX`, `GT`, `LT` with Valkey's errors.
    pub(crate) fn parse(args: &[Bytes]) -> Result<Cond, Value> {
        let mut c = Cond::default();
        for a in args {
            match upper(a).as_str() {
                "NX" => c.nx = true,
                "XX" => c.xx = true,
                "GT" => c.gt = true,
                "LT" => c.lt = true,
                _ => {
                    return Err(Value::err(format!(
                        "ERR Unsupported option {}",
                        String::from_utf8_lossy(a)
                    )));
                }
            }
        }
        if c.nx && (c.xx || c.gt || c.lt) {
            return Err(Value::err(
                "ERR NX and XX, GT or LT options at the same time are not compatible",
            ));
        }
        if c.gt && c.lt {
            return Err(Value::err(
                "ERR GT and LT options at the same time are not compatible",
            ));
        }
        Ok(c)
    }

    /// Whether a new expiry `when` (Unix ms, possibly negative) applies over
    /// the current `current` (0: none, which `GT` and `LT` read as
    /// infinite).
    pub(crate) fn allows(self, current: u64, when: i64) -> bool {
        let current = (current != 0).then(|| i64::try_from(current).unwrap_or(i64::MAX));
        if self.nx && current.is_some() {
            return false;
        }
        if self.xx && current.is_none() {
            return false;
        }
        if self.gt && current.is_none_or(|c| when <= c) {
            return false;
        }
        if self.lt && current.is_some_and(|c| when >= c) {
            return false;
        }
        true
    }
}

/// The absolute expiry, in Unix ms, of `value` in `unit_ms` units, relative
/// to `now_ms` unless `absolute`; Valkey's error on overflow.
pub(crate) fn expire_when(
    value: i64,
    unit_ms: i64,
    absolute: bool,
    now_ms: u64,
    command: &str,
) -> Result<i64, Value> {
    let invalid = || Value::err(format!("ERR invalid expire time in '{command}' command"));
    let ms = value.checked_mul(unit_ms).ok_or_else(invalid)?;
    if absolute {
        return Ok(ms);
    }
    let now = i64::try_from(now_ms).map_err(|_| invalid())?;
    ms.checked_add(now).ok_or_else(invalid)
}

async fn expire(
    shared: &Shared,
    db: u8,
    args: &[Bytes],
    unit_ms: i64,
    absolute: bool,
    command: &'static str,
) -> Result<Value, Value> {
    let value = int_arg(&args[2])?;
    let cond = Cond::parse(&args[3..])?;
    // Overflow is refused before the key is read, as Valkey does.
    expire_when(value, unit_ms, absolute, types::now_ms(), command)?;
    let key = args[1].clone();
    let applied = txn(shared, "resp.expire", move |t| {
        let key = key.clone();
        Box::pin(async move {
            let now = types::now_ms();
            let Ok(when) = expire_when(value, unit_ms, absolute, now, command) else {
                return Ok(false);
            };
            set_expiry(t, db, &key, cond, when, now).await
        })
    })
    .await?;
    Ok(Value::Integer(i64::from(applied)))
}

/// Applies an expiry to `ukey` under `cond`: in the past, the key is
/// deleted; otherwise its meta record gets `when`. Returns whether it
/// applied (the key is live and the condition holds).
pub(crate) async fn set_expiry<K: Kv>(
    t: &mut K,
    db: u8,
    ukey: &[u8],
    cond: Cond,
    when: i64,
    now: u64,
) -> Result<bool, loams_kv::TxnError> {
    let found = read_live(t, db, ukey, now).await?;
    let Some(mut meta) = found.live else {
        return Ok(false);
    };
    if !cond.allows(meta.expire_at_ms, when) {
        return Ok(false);
    }
    match u64::try_from(when) {
        Ok(at) if at > now => {
            meta.expire_at_ms = at;
            put_meta(t, db, ukey, &meta).await?;
        }
        _ => {
            delete_key(t, db, ukey, now).await?;
        }
    }
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ttl {
    Seconds,
    Millis,
    AtSeconds,
    AtMillis,
}

async fn ttl(shared: &Shared, db: u8, key: Bytes, form: Ttl) -> Result<Value, Value> {
    let (meta, now) = txn(shared, "resp.ttl", move |t| {
        let key = key.clone();
        Box::pin(async move {
            let now = types::now_ms();
            Ok((read_live(t, db, &key, now).await?.live, now))
        })
    })
    .await?;
    Ok(Value::Integer(ttl_reply(meta.as_ref(), now, form)))
}

fn ttl_reply(meta: Option<&Meta>, now: u64, form: Ttl) -> i64 {
    let Some(meta) = meta else {
        return -2;
    };
    if meta.expire_at_ms == 0 {
        return -1;
    }
    let at = i64::try_from(meta.expire_at_ms).unwrap_or(i64::MAX);
    let left = i64::try_from(meta.expire_at_ms.saturating_sub(now)).unwrap_or(i64::MAX);
    match form {
        Ttl::Seconds => (left + 500) / 1000,
        Ttl::Millis => left,
        Ttl::AtSeconds => at / 1000,
        Ttl::AtMillis => at,
    }
}

async fn persist(shared: &Shared, db: u8, key: Bytes) -> Result<Value, Value> {
    let done = txn(shared, "resp.persist", move |t| {
        let key = key.clone();
        Box::pin(async move {
            let found = read_live(t, db, &key, types::now_ms()).await?;
            match found.live {
                Some(mut meta) if meta.expire_at_ms != 0 => {
                    meta.expire_at_ms = 0;
                    put_meta(t, db, &key, &meta).await?;
                    Ok(true)
                }
                _ => Ok(false),
            }
        })
    })
    .await?;
    Ok(Value::Integer(i64::from(done)))
}

/// `DBSIZE`: a counted scan of the database's meta range, one transaction
/// per page (O(n), documented; expired keys not yet reclaimed are left
/// out).
async fn dbsize(shared: &Shared, db: u8) -> Result<Value, Value> {
    let (mut start, end) = layout::meta_range(db);
    let mut total = 0i64;
    loop {
        let from = start.clone();
        let end = end.clone();
        let (live, last, full) = txn(shared, "resp.dbsize", move |t| {
            let (from, end) = (from.clone(), end.clone());
            Box::pin(async move {
                let now = types::now_ms();
                let pairs = t.scan(&from, Some(&end), DBSIZE_PAGE).await?;
                let full = pairs.len() == DBSIZE_PAGE;
                let mut live = 0i64;
                for (_, v) in &pairs {
                    let meta = Meta::decode(v).map_err(|e| types::corrupt(&e))?;
                    if !meta.expired(now) {
                        live += 1;
                    }
                }
                Ok((live, pairs.last().map(|(k, _)| k.clone()), full))
            })
        })
        .await?;
        total += live;
        match last {
            Some(mut k) if full => {
                k.push(0);
                start = k;
            }
            _ => return Ok(Value::Integer(total)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Bytes {
        Bytes::from(s.to_owned())
    }

    #[test]
    fn expire_conditions() {
        let parse = |opts: &[&str]| Cond::parse(&opts.iter().map(|o| b(*o)).collect::<Vec<_>>());
        assert!(parse(&["NX", "XX"]).is_err());
        assert!(parse(&["GT", "LT"]).is_err());
        assert!(parse(&["FOO"]).is_err());
        let nx = parse(&["nx"]).expect("NX");
        assert!(nx.allows(0, 5));
        assert!(!nx.allows(10, 5));
        let xx = parse(&["XX"]).expect("XX");
        assert!(!xx.allows(0, 5));
        assert!(xx.allows(10, 5));
        let gt = parse(&["GT"]).expect("GT");
        assert!(!gt.allows(0, 5), "no TTL is infinite");
        assert!(gt.allows(10, 11));
        assert!(!gt.allows(10, 10));
        let lt = parse(&["LT"]).expect("LT");
        assert!(lt.allows(0, 5), "no TTL is infinite");
        assert!(lt.allows(10, 9));
        assert!(!lt.allows(10, 10));
        let xx_gt = parse(&["XX", "GT"]).expect("XX GT");
        assert!(!xx_gt.allows(0, 50));
        assert!(xx_gt.allows(10, 50));
    }

    #[test]
    fn expire_overflow_is_refused() {
        assert!(expire_when(i64::MAX / 999, 1000, false, 0, "expire").is_err());
        assert!(expire_when(i64::MAX - 5, 1, false, 10, "pexpire").is_err());
        assert_eq!(expire_when(5, 1000, false, 100, "expire"), Ok(5100));
        assert_eq!(expire_when(-1, 1000, true, 100, "expireat"), Ok(-1000));
    }

    #[test]
    fn ttl_replies() {
        let mut m = Meta::string(1, 0, &layout::StringBody::Inline(vec![]));
        assert_eq!(ttl_reply(None, 0, Ttl::Seconds), -2);
        assert_eq!(ttl_reply(Some(&m), 0, Ttl::Millis), -1);
        m.expire_at_ms = 10_600;
        assert_eq!(ttl_reply(Some(&m), 1_000, Ttl::Seconds), 10);
        assert_eq!(ttl_reply(Some(&m), 1_000, Ttl::Millis), 9_600);
        assert_eq!(ttl_reply(Some(&m), 1_000, Ttl::AtSeconds), 10);
        assert_eq!(ttl_reply(Some(&m), 1_000, Ttl::AtMillis), 10_600);
    }
}
