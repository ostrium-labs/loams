//! Server commands: `TIME`, `INFO`, `CONFIG`, `DEBUG`, `COMMAND`,
//! `FUNCTION`/`SCRIPT FLUSH`, `FLUSHALL`, `FLUSHDB`, `LASTSAVE` (plan KV2
//! Task 2). `CONFIG` is tolerant: it accepts and echoes parameters it does
//! not use, which Valkey's suite needs for its per-block overrides
//! (report §1.2). `DEBUG` answers the few subcommands the suite calls
//! outside `needs:debug` blocks.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bytes::Bytes;
use loams_kv::{Store, TxnError, TxnOptions};

use super::{
    COMMANDS, Spec, arity_ok, command_info, keys_of, lookup, syntax_error, unknown_subcommand,
    upper, wrong_arity,
};
use crate::frame::Value;
use crate::server::{Conn, Shared, unix_now, verbatim};

/// The keys one flush transaction deletes at most.
const FLUSH_BATCH: usize = 256;

pub(super) async fn run(
    shared: &Arc<Shared>,
    conn: &mut Conn,
    spec: &Spec,
    args: &[Bytes],
) -> Value {
    match spec.name {
        "time" => {
            let (secs, micros) = unix_now();
            Value::Array(vec![
                Value::bulk(secs.to_string()),
                Value::bulk(micros.to_string()),
            ])
        }
        "lastsave" => Value::Integer(i64::try_from(unix_now().0).unwrap_or(0)),
        "readwrite" | "readonly" => Value::ok(),
        "info" => verbatim(info(shared, args)),
        "config" => config(shared, args),
        "debug" => debug(args).await,
        "command" => command(args),
        "function" | "script" => match upper(&args[1]).as_str() {
            "FLUSH" if args.len() <= 3 => Value::ok(),
            _ => Value::err(format!(
                "ERR '{} {}' is not supported by Loams",
                spec.name,
                String::from_utf8_lossy(&args[1]).to_ascii_lowercase()
            )),
        },
        "flushall" | "flushdb" => {
            if args.len() > 2 {
                return syntax_error();
            }
            if let Some(mode) = args.get(1)
                && !matches!(upper(mode).as_str(), "SYNC" | "ASYNC")
            {
                return syntax_error();
            }
            // Keys of database `db` start with the byte `db` (D1653).
            // FLUSHALL takes every database's range; leases and the GC queue
            // sort after the database bytes (MAX_DATABASES, ruling K2-8) and
            // are left to their workers.
            let (start, end) = if spec.name == "flushall" {
                (0u8, u8::try_from(shared.databases).ok())
            } else {
                (conn.db, conn.db.checked_add(1))
            };
            let end = end.map(|e| vec![e]);
            match delete_range(&shared.store, &[start], end.as_deref()).await {
                Ok(()) => Value::ok(),
                Err(e) => store_error(&e),
            }
        }
        other => Value::err(format!("ERR '{other}' has no handler")),
    }
}

/// The reply for a store error that reached a handler.
pub fn store_error(e: &TxnError) -> Value {
    Value::err(format!("ERR storage error: {e}"))
}

/// Deletes every key in `start..end`, in transactions of at most
/// [`FLUSH_BATCH`] keys. Not atomic as a whole: a concurrent writer may
/// see a partly flushed range (a documented deviation from Valkey).
pub async fn delete_range(store: &Store, start: &[u8], end: Option<&[u8]>) -> Result<(), TxnError> {
    loop {
        let (s, e) = (start.to_vec(), end.map(<[u8]>::to_vec));
        let deleted = store
            .run(TxnOptions::new("resp.flush"), move |txn| {
                let (s, e) = (s.clone(), e.clone());
                Box::pin(async move {
                    let pairs = txn.scan(&s, e.as_deref(), FLUSH_BATCH).await?;
                    for (key, _) in &pairs {
                        txn.delete(key).await?;
                    }
                    Ok(pairs.len())
                })
            })
            .await?
            .value;
        if deleted < FLUSH_BATCH {
            return Ok(());
        }
    }
}

fn info(shared: &Shared, args: &[Bytes]) -> String {
    let wanted: Vec<String> = args[1..]
        .iter()
        .map(|a| String::from_utf8_lossy(a).to_ascii_lowercase())
        .collect();
    let all = wanted.is_empty()
        || wanted
            .iter()
            .any(|w| matches!(w.as_str(), "all" | "default" | "everything"));
    let mut out = String::new();
    let mut section = |name: &str, body: String| {
        let lower = name.to_ascii_lowercase();
        if all || wanted.contains(&lower) {
            if !out.is_empty() {
                out.push_str("\r\n");
            }
            out.push_str("# ");
            out.push_str(name);
            out.push_str("\r\n");
            out.push_str(&body);
        }
    };
    let uptime = shared.started.elapsed().as_secs();
    section(
        "Server",
        format!(
            "redis_version:{}\r\nloams_version:{}\r\nserver_name:loams\r\nredis_mode:standalone\r\nos:{}\r\narch_bits:64\r\ntcp_port:{}\r\nuptime_in_seconds:{uptime}\r\nuptime_in_days:{}\r\nhz:10\r\nexecutable:loams-gateway\r\n",
            super::REDIS_VERSION,
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            shared.listen.port(),
            uptime / 86_400,
        ),
    );
    let clients = shared.clients.lock().map(|c| c.len()).unwrap_or_default();
    section(
        "Clients",
        format!(
            "connected_clients:{clients}\r\nmaxclients:{}\r\nblocked_clients:0\r\ntracking_clients:0\r\n",
            shared.config.max_clients
        ),
    );
    section(
        "Memory",
        "used_memory:0\r\nused_memory_human:0B\r\nmaxmemory:0\r\nmaxmemory_policy:noeviction\r\n"
            .into(),
    );
    section(
        "Persistence",
        "loading:0\r\nrdb_changes_since_last_save:0\r\nrdb_bgsave_in_progress:0\r\naof_enabled:0\r\naof_rewrite_in_progress:0\r\n"
            .into(),
    );
    section(
        "Stats",
        format!(
            "total_connections_received:{}\r\ntotal_commands_processed:{}\r\nrejected_connections:{}\r\nexpired_keys:0\r\nevicted_keys:0\r\nkeyspace_hits:0\r\nkeyspace_misses:0\r\n",
            shared.stats.connections.load(Ordering::Relaxed),
            shared.stats.commands.load(Ordering::Relaxed),
            shared.stats.rejected.load(Ordering::Relaxed),
        ),
    );
    section(
        "Replication",
        "role:master\r\nconnected_slaves:0\r\nmaster_repl_offset:0\r\n".into(),
    );
    section("CPU", "used_cpu_sys:0.0\r\nused_cpu_user:0.0\r\n".into());
    section("Modules", String::new());
    section("Errorstats", String::new());
    // The key counts need the key layout (Task 3); until then the section
    // is present and empty.
    section("Keyspace", String::new());
    out
}

/// Defaults `CONFIG GET` reports for parameters never set.
const DEFAULTS: &[(&str, &str)] = &[
    ("save", ""),
    ("appendonly", "no"),
    ("maxmemory", "0"),
    ("maxmemory-policy", "noeviction"),
    ("timeout", "0"),
    ("hz", "10"),
    ("lazyfree-lazy-user-del", "no"),
    ("lazyfree-lazy-user-flush", "no"),
    ("list-max-listpack-size", "-2"),
    ("list-max-ziplist-size", "-2"),
    ("list-compress-depth", "0"),
    ("set-max-intset-entries", "512"),
    ("set-max-listpack-entries", "128"),
    ("hash-max-listpack-entries", "128"),
    ("hash-max-ziplist-entries", "128"),
    ("zset-max-listpack-entries", "128"),
    ("zset-max-ziplist-entries", "128"),
    ("notify-keyspace-events", ""),
    ("appendfsync", "everysec"),
    ("busy-reply-threshold", "5000"),
    ("lua-time-limit", "5000"),
    ("latency-tracking", "no"),
];

fn config(shared: &Shared, args: &[Bytes]) -> Value {
    let sub = upper(&args[1]);
    match sub.as_str() {
        "GET" if args.len() >= 3 => {
            let params = shared.params.lock().unwrap_or_else(|e| e.into_inner());
            let fixed = [
                ("databases", shared.databases.to_string()),
                ("maxclients", shared.config.max_clients.to_string()),
                (
                    "proto-max-bulk-len",
                    shared.config.proto_max_bulk_len.to_string(),
                ),
                ("port", shared.listen.port().to_string()),
            ];
            let mut pairs: Vec<(String, String)> = Vec::new();
            for pattern in &args[2..] {
                let pattern = String::from_utf8_lossy(pattern).to_ascii_lowercase();
                let candidates = params
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .chain(fixed.iter().map(|(k, v)| (*k, v.as_str())))
                    .chain(DEFAULTS.iter().copied());
                for (k, v) in candidates {
                    if glob(pattern.as_bytes(), k.as_bytes()) && !pairs.iter().any(|(x, _)| x == k)
                    {
                        pairs.push((k.to_owned(), v.to_owned()));
                    }
                }
            }
            Value::Map(
                pairs
                    .into_iter()
                    .map(|(k, v)| (Value::bulk(k), Value::bulk(v)))
                    .collect(),
            )
        }
        "SET" if args.len() >= 4 && args.len().is_multiple_of(2) => {
            let mut params = shared.params.lock().unwrap_or_else(|e| e.into_inner());
            for pair in args[2..].chunks(2) {
                params.insert(
                    String::from_utf8_lossy(&pair[0]).to_ascii_lowercase(),
                    String::from_utf8_lossy(&pair[1]).into_owned(),
                );
            }
            Value::ok()
        }
        "RESETSTAT" if args.len() == 2 => Value::ok(),
        "REWRITE" if args.len() == 2 => {
            Value::err("ERR The server is running without a config file")
        }
        "GET" | "SET" | "RESETSTAT" | "REWRITE" => Value::err(format!(
            "ERR wrong number of arguments for 'config|{}' command",
            sub.to_ascii_lowercase()
        )),
        _ => unknown_subcommand("config", &args[1]),
    }
}

async fn debug(args: &[Bytes]) -> Value {
    let sub = upper(&args[1]);
    match sub.as_str() {
        "SET-ACTIVE-EXPIRE" | "QUICKLIST-PACKED-THRESHOLD" | "SET-DISABLE-DENY-SCRIPTS" => {
            Value::ok()
        }
        "SLEEP" if args.len() == 3 => {
            let secs: f64 = String::from_utf8_lossy(&args[2]).parse().unwrap_or(0.0);
            if secs > 0.0 && secs.is_finite() {
                tokio::time::sleep(Duration::from_secs_f64(secs.min(60.0))).await;
            }
            Value::ok()
        }
        _ => Value::err(format!("ERR DEBUG {sub} is not supported by Loams")),
    }
}

fn command(args: &[Bytes]) -> Value {
    if args.len() == 1 {
        return Value::Array(COMMANDS.iter().map(command_info).collect());
    }
    let sub = upper(&args[1]);
    match sub.as_str() {
        "COUNT" if args.len() == 2 => Value::Integer(i64::try_from(COMMANDS.len()).unwrap_or(0)),
        "LIST" => Value::Array(COMMANDS.iter().map(|s| Value::bulk(s.name)).collect()),
        "INFO" => {
            if args.len() == 2 {
                return Value::Array(COMMANDS.iter().map(command_info).collect());
            }
            Value::Array(
                args[2..]
                    .iter()
                    .map(|n| lookup(n).map_or(Value::NullArray, command_info))
                    .collect(),
            )
        }
        "DOCS" => {
            let specs: Vec<&Spec> = if args.len() == 2 {
                COMMANDS.iter().collect()
            } else {
                args[2..]
                    .iter()
                    .filter_map(|n| lookup(n.as_ref()))
                    .collect()
            };
            Value::Map(
                specs
                    .into_iter()
                    .map(|s| {
                        (
                            Value::bulk(s.name),
                            Value::Map(vec![(Value::bulk("summary"), Value::bulk(""))]),
                        )
                    })
                    .collect(),
            )
        }
        "GETKEYS" if args.len() >= 3 => {
            let Some(spec) = lookup(&args[2]) else {
                return Value::err("ERR Invalid command specified");
            };
            let request = &args[2..];
            if !arity_ok(spec.arity, request.len()) {
                return Value::err("ERR Invalid number of arguments specified for command");
            }
            let keys = keys_of(spec, request);
            if keys.is_empty() {
                return Value::err("ERR The command has no key arguments");
            }
            Value::Array(keys.into_iter().map(|k| Value::Bulk(k.clone())).collect())
        }
        "COUNT" | "GETKEYS" => wrong_arity(&format!("command|{}", sub.to_ascii_lowercase())),
        _ => unknown_subcommand("command", &args[1]),
    }
}

/// Valkey's glob: `*`, `?`, `[…]` with ranges and `^`, and `\` escapes.
/// Re-implemented from the documented semantics, not ported.
pub fn glob(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let (mut star_p, mut star_t) = (None, 0);
    while t < text.len() {
        if p < pattern.len() {
            let step = match pattern[p] {
                b'*' => {
                    while p < pattern.len() && pattern[p] == b'*' {
                        p += 1;
                    }
                    if p == pattern.len() {
                        return true;
                    }
                    star_p = Some(p);
                    star_t = t;
                    continue;
                }
                b'?' => Some(p + 1),
                b'[' => class(pattern, p, text[t]).and_then(|(m, next)| m.then_some(next)),
                b'\\' if p + 1 < pattern.len() => (pattern[p + 1] == text[t]).then_some(p + 2),
                c => (c == text[t]).then_some(p + 1),
            };
            if let Some(next) = step {
                p = next;
                t += 1;
                continue;
            }
        }
        match star_p {
            Some(sp) => {
                star_t += 1;
                t = star_t;
                p = sp;
            }
            None => return false,
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

/// A `[…]` class at `pattern[p]`: whether `c` matches, and the index after
/// the class. An unterminated class runs to the end of the pattern.
fn class(pattern: &[u8], p: usize, c: u8) -> Option<(bool, usize)> {
    let mut i = p + 1;
    let negate = pattern.get(i) == Some(&b'^');
    if negate {
        i += 1;
    }
    let mut matched = false;
    while i < pattern.len() && pattern[i] != b']' {
        if pattern[i] == b'\\' && i + 1 < pattern.len() {
            matched |= pattern[i + 1] == c;
            i += 2;
        } else if i + 2 < pattern.len() && pattern[i + 1] == b'-' && pattern[i + 2] != b']' {
            let (lo, hi) = (
                pattern[i].min(pattern[i + 2]),
                pattern[i].max(pattern[i + 2]),
            );
            matched |= (lo..=hi).contains(&c);
            i += 3;
        } else {
            matched |= pattern[i] == c;
            i += 1;
        }
    }
    let next = if i < pattern.len() { i + 1 } else { i };
    Some((matched != negate, next))
}

#[cfg(test)]
mod tests {
    use super::glob;

    #[test]
    fn glob_cases() {
        for (p, t, want) in [
            ("*", "anything", true),
            ("h?llo", "hello", true),
            ("h?llo", "hllo", false),
            ("h*llo", "heeeello", true),
            ("h[ae]llo", "hallo", true),
            ("h[ae]llo", "hillo", false),
            ("h[^e]llo", "hallo", true),
            ("h[^e]llo", "hello", false),
            ("h[a-b]llo", "hbllo", true),
            ("h\\*llo", "h*llo", true),
            ("h\\*llo", "hallo", false),
            ("list-max-*", "list-max-ziplist-size", true),
            ("maxmemory", "maxmemory-policy", false),
            ("*a*b", "xaxxb", true),
            ("", "", true),
            ("", "a", false),
        ] {
            assert_eq!(glob(p.as_bytes(), t.as_bytes()), want, "{p} ~ {t}");
        }
    }
}
