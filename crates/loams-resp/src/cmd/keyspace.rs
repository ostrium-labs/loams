//! Keyspace commands (plan KV2 Task 9): `KEYS`, `SCAN` (an opaque cursor
//! mapped to a resume key per connection, `MATCH`, `COUNT`, `TYPE`),
//! `RANDOMKEY`, `RENAME`, `RENAMENX`, `COPY`, `OBJECT`. Glob matching is
//! re-implemented from Valkey's documented semantics (`cmd::glob`).

use std::sync::Arc;

use bytes::Bytes;

use super::hash::{Rng, scan_reply};
use super::{Spec, glob, int_arg, store_error, syntax_error, txn, unknown_subcommand, upper};
use crate::cursor::{self, Resume};
use crate::frame::{Value, parse_int};
use crate::layout::{Kind, StringBody};
use crate::server::{Conn, Shared};
use crate::types::keyspace::{self, Moved};
use crate::types::{now_ms, read_live};

/// The meta keys one `KEYS` transaction reads.
const KEYS_PAGE: usize = 1024;

pub(super) async fn run(
    shared: &Arc<Shared>,
    conn: &mut Conn,
    spec: &Spec,
    args: &[Bytes],
) -> Value {
    let result = match spec.name {
        "keys" => keys(shared, conn.db, args[1].clone()).await,
        "scan" => scan(shared, conn, args).await,
        "randomkey" => randomkey(shared, conn).await,
        "rename" | "renamenx" => rename(shared, conn.db, args, spec.name == "renamenx").await,
        "copy" => copy(shared, conn.db, args).await,
        "object" => object(shared, conn.db, args).await,
        other => Err(Value::err(format!("ERR '{other}' has no handler"))),
    };
    result.unwrap_or_else(|e| e)
}

/// `KEYS pattern`: every live key that matches, paging through the meta
/// range (O(n), as in Valkey).
async fn keys(shared: &Shared, db: u8, pattern: Bytes) -> Result<Value, Value> {
    let mut out = Vec::new();
    let mut from: Option<Vec<u8>> = None;
    loop {
        let start = from.clone();
        let (live, next) = txn(shared, "resp.keys", move |t| {
            let start = start.clone();
            Box::pin(
                async move { keyspace::page(t, db, start.as_deref(), KEYS_PAGE, now_ms()).await },
            )
        })
        .await?;
        out.extend(
            live.into_iter()
                .filter(|(k, _)| glob(&pattern, k))
                .map(|(k, _)| Value::bulk(k)),
        );
        match next {
            Some(n) => from = Some(n),
            None => return Ok(Value::Array(out)),
        }
    }
}

/// The `TYPE` filter's kind.
fn kind_named(name: &str) -> Option<Kind> {
    Some(match name {
        "string" => Kind::String,
        "hash" => Kind::Hash,
        "set" => Kind::Set,
        "zset" => Kind::Zset,
        "list" => Kind::List,
        _ => return None,
    })
}

/// `SCAN cursor [MATCH pattern] [COUNT count] [TYPE type]`. Every key
/// present for the whole iteration is returned at least once: the cursor
/// resumes at a user key, and keys are visited in key order.
async fn scan(shared: &Shared, conn: &mut Conn, args: &[Bytes]) -> Result<Value, Value> {
    let id = cursor::parse(&args[1])?;
    let (mut pattern, mut count, mut kind) = (None, 10usize, None);
    let mut type_filter = false;
    let mut i = 2;
    while i < args.len() {
        let Some(arg) = args.get(i + 1) else {
            return Err(syntax_error());
        };
        match upper(&args[i]).as_str() {
            "MATCH" => pattern = Some(arg.clone()),
            "COUNT" => {
                let n = int_arg(arg)?;
                if n < 1 {
                    return Err(syntax_error());
                }
                count = usize::try_from(n).unwrap_or(usize::MAX).min(100_000);
            }
            "TYPE" => {
                // An unknown type matches nothing, as in Valkey.
                type_filter = true;
                kind = kind_named(&String::from_utf8_lossy(arg).to_ascii_lowercase());
            }
            _ => return Err(syntax_error()),
        }
        i += 2;
    }
    let db = conn.db;
    let scope = [b"scan\0".as_slice(), &[db]].concat();
    let from = match conn.cursors.take(id, &scope) {
        Resume::Start => None,
        Resume::From(k) => Some(k),
        Resume::Unknown => return Ok(scan_reply(0, Vec::new())),
    };
    let (live, next) = txn(shared, "resp.scan", move |t| {
        let from = from.clone();
        Box::pin(async move { keyspace::page(t, db, from.as_deref(), count, now_ms()).await })
    })
    .await?;
    let items = live
        .into_iter()
        .filter(|(k, _)| pattern.as_ref().is_none_or(|p| glob(p, k)))
        .filter(|(_, m)| !type_filter || Some(m.kind) == kind)
        .map(|(k, _)| Value::bulk(k))
        .collect();
    let cursor = next.map_or(0, |k| conn.cursors.save(&scope, k));
    Ok(scan_reply(cursor, items))
}

/// `RANDOMKEY`: the first live key at or after a random point of the meta
/// range (not a full load, distributedio/titan#230; not uniform over keys,
/// documented).
async fn randomkey(shared: &Shared, conn: &Conn) -> Result<Value, Value> {
    let db = conn.db;
    let mut rng = Rng::seeded(conn.id);
    let start = rng.next_u64().to_be_bytes().to_vec();
    let found = txn(shared, "resp.randomkey", move |t| {
        let start = start.clone();
        Box::pin(async move { keyspace::key_from(t, db, &start, 64, now_ms()).await })
    })
    .await?;
    Ok(found.map_or(Value::Null, Value::bulk))
}

fn moved_error(m: Moved, command: &str) -> Value {
    match m {
        Moved::NoSource => Value::err("ERR no such key"),
        Moved::TooLarge => Value::err(format!(
            "ERR {} of a key with more than {} elements is not supported by Loams",
            command.to_ascii_uppercase(),
            keyspace::MOVE_MAX_KEYS
        )),
        Moved::Done | Moved::DestinationExists => {
            store_error(&loams_kv::TxnError::Fatal("unexpected move outcome".into()))
        }
    }
}

/// `RENAME src dst` (`OK`) and `RENAMENX src dst` (1, or 0 when `dst`
/// exists).
async fn rename(shared: &Shared, db: u8, args: &[Bytes], nx: bool) -> Result<Value, Value> {
    let (src, dst) = (args[1].clone(), args[2].clone());
    let command = if nx { "renamenx" } else { "rename" };
    let moved = txn(shared, "resp.rename", move |t| {
        let (src, dst) = (src.clone(), dst.clone());
        Box::pin(async move { keyspace::copy(t, db, &src, db, &dst, !nx, true, now_ms()).await })
    })
    .await?;
    match moved {
        Moved::Done if nx && args[1] == args[2] => Ok(Value::Integer(0)),
        Moved::Done if nx => Ok(Value::Integer(1)),
        Moved::Done => Ok(Value::ok()),
        Moved::DestinationExists => Ok(Value::Integer(0)),
        other => Err(moved_error(other, command)),
    }
}

/// `COPY src dst [DB db] [REPLACE]`: 1 when copied, 0 when `dst` exists
/// (without `REPLACE`) or `src` does not.
async fn copy(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    let (mut dst_db, mut replace) = (db, false);
    let mut i = 3;
    while i < args.len() {
        match upper(&args[i]).as_str() {
            "REPLACE" => {
                replace = true;
                i += 1;
            }
            "DB" if i + 1 < args.len() => {
                let n = int_arg(&args[i + 1])?;
                if n < 0 || n >= i64::from(shared.databases) {
                    return Err(Value::err("ERR DB index is out of range"));
                }
                dst_db = u8::try_from(n).unwrap_or(0);
                i += 2;
            }
            _ => return Err(syntax_error()),
        }
    }
    if dst_db == db && args[1] == args[2] {
        return Err(Value::err(
            "ERR source and destination objects are the same",
        ));
    }
    let (src, dst) = (args[1].clone(), args[2].clone());
    let moved = txn(shared, "resp.copy", move |t| {
        let (src, dst) = (src.clone(), dst.clone());
        Box::pin(async move {
            keyspace::copy(t, db, &src, dst_db, &dst, replace, false, now_ms()).await
        })
    })
    .await?;
    match moved {
        Moved::Done => Ok(Value::Integer(1)),
        Moved::NoSource | Moved::DestinationExists => Ok(Value::Integer(0)),
        Moved::TooLarge => Err(moved_error(moved, "copy")),
    }
}

/// `OBJECT ENCODING` answers a fixed encoding per type (strings by size
/// and content, as Valkey's thresholds would); `REFCOUNT` is 1; `FREQ`
/// and `IDLETIME` are refused.
async fn object(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    let sub = upper(&args[1]);
    match (sub.as_str(), args.len()) {
        ("HELP", 2) => Ok(Value::Array(
            [
                "OBJECT <subcommand> [<arg> [value] [opt] ...]. Subcommands are:",
                "ENCODING <key>",
                "    Return the kind of internal representation used in order to store the value",
                "    associated with a <key>.",
                "REFCOUNT <key>",
                "    Return the number of references of the value associated with the specified",
                "    <key>.",
            ]
            .into_iter()
            .map(|l| Value::Simple(l.to_owned()))
            .collect(),
        )),
        ("FREQ" | "IDLETIME", 3) => Err(Value::err(format!(
            "ERR OBJECT {sub} is not supported by Loams"
        ))),
        ("ENCODING" | "REFCOUNT", 3) => {
            let key = args[2].clone();
            let meta = txn(shared, "resp.object", move |t| {
                let key = key.clone();
                Box::pin(async move { Ok(read_live(t, db, &key, now_ms()).await?.live) })
            })
            .await?;
            let Some(meta) = meta else {
                return Ok(Value::Null);
            };
            if sub == "REFCOUNT" {
                return Ok(Value::Integer(1));
            }
            let encoding = match meta.kind {
                Kind::String => match meta.string_body() {
                    Some(StringBody::Inline(b)) if b.len() <= 20 && parse_int(&b).is_some() => {
                        "int"
                    }
                    Some(StringBody::Inline(b)) if b.len() <= 44 => "embstr",
                    _ => "raw",
                },
                Kind::Hash => "hashtable",
                Kind::Set => "hashtable",
                Kind::Zset => "skiplist",
                Kind::List => "quicklist",
            };
            Ok(Value::bulk(encoding))
        }
        _ => Err(unknown_subcommand("object", &args[1])),
    }
}

#[cfg(test)]
mod tests {
    use super::super::glob;

    /// Glob cases from Valkey's documented semantics (`KEYS`).
    #[test]
    fn glob_matches_valkey_cases() {
        for (p, t, want) in [
            ("h?llo", "hello", true),
            ("h?llo", "hallo", true),
            ("h*llo", "hllo", true),
            ("h*llo", "heeeello", true),
            ("h[ae]llo", "hello", true),
            ("h[ae]llo", "hillo", false),
            ("h[^e]llo", "hallo", true),
            ("h[^e]llo", "hello", false),
            ("h[a-b]llo", "hallo", true),
            ("h[a-b]llo", "hcllo", false),
            ("h[b-a]llo", "hallo", true),
            ("*", "", true),
            ("*", "anything at all", true),
            ("a*", "b", false),
            ("*a", "ba", true),
            ("a\\*b", "a*b", true),
            ("a\\*b", "axb", false),
            ("[\\]]", "]", true),
            ("foo*bar*baz", "foo-bar-x-baz", true),
            ("foo*bar*baz", "foo-baz-bar", false),
            ("key:??", "key:12", true),
            ("key:??", "key:1", false),
        ] {
            assert_eq!(glob(p.as_bytes(), t.as_bytes()), want, "{p} ~ {t}");
        }
    }
}
