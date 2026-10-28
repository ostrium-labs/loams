//! The command table and dispatch (plan KV2 Task 2).
//!
//! Each command has a [`Spec`]: its name, Valkey's arity (positive: exact;
//! negative: at least), flags and key positions, which `COMMAND` reports
//! and dispatch checks before running a handler. Unknown and out-of-scope
//! commands get Valkey's error texts (design §77 §8.5).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use bytes::Bytes;

use crate::frame::{Value, parse_int};
use crate::server::{Conn, Shared};

mod conn;
mod server;

pub use conn::REDIS_VERSION;
pub use server::{delete_range, glob, store_error};

/// What the connection does after a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// Write the reply, then close (`QUIT`).
    Close,
}

/// Which handler family runs a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Conn,
    Server,
}

/// Command flags, as `COMMAND INFO` names them.
pub mod flag {
    pub const WRITE: u16 = 1;
    pub const READONLY: u16 = 1 << 1;
    pub const FAST: u16 = 1 << 2;
    pub const NOAUTH: u16 = 1 << 3;
    pub const LOADING: u16 = 1 << 4;
    pub const STALE: u16 = 1 << 5;
    pub const ADMIN: u16 = 1 << 6;
    pub const NOSCRIPT: u16 = 1 << 7;
    pub const DENYOOM: u16 = 1 << 8;

    /// The names `COMMAND INFO` reports.
    pub const NAMES: &[(u16, &str)] = &[
        (WRITE, "write"),
        (READONLY, "readonly"),
        (DENYOOM, "denyoom"),
        (ADMIN, "admin"),
        (NOSCRIPT, "noscript"),
        (LOADING, "loading"),
        (STALE, "stale"),
        (FAST, "fast"),
        (NOAUTH, "no_auth"),
    ];
}

/// One command.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    /// Lower case, as Valkey reports it.
    pub name: &'static str,
    pub arity: i32,
    pub flags: u16,
    pub first_key: i32,
    pub last_key: i32,
    pub step: i32,
    pub family: Family,
}

const fn spec(
    name: &'static str,
    arity: i32,
    flags: u16,
    keys: (i32, i32, i32),
    family: Family,
) -> Spec {
    Spec {
        name,
        arity,
        flags,
        first_key: keys.0,
        last_key: keys.1,
        step: keys.2,
        family,
    }
}

use flag::*;
const NOKEYS: (i32, i32, i32) = (0, 0, 0);

/// Every command this build serves.
pub static COMMANDS: &[Spec] = &[
    spec("ping", -1, FAST | STALE, NOKEYS, Family::Conn),
    spec("echo", 2, FAST, NOKEYS, Family::Conn),
    spec(
        "hello",
        -1,
        NOAUTH | FAST | STALE | LOADING | NOSCRIPT,
        NOKEYS,
        Family::Conn,
    ),
    spec(
        "auth",
        -2,
        NOAUTH | FAST | STALE | LOADING | NOSCRIPT,
        NOKEYS,
        Family::Conn,
    ),
    spec("select", 2, FAST | LOADING | STALE, NOKEYS, Family::Conn),
    spec(
        "reset",
        1,
        NOAUTH | FAST | STALE | LOADING | NOSCRIPT,
        NOKEYS,
        Family::Conn,
    ),
    spec(
        "quit",
        -1,
        NOAUTH | FAST | STALE | LOADING | NOSCRIPT,
        NOKEYS,
        Family::Conn,
    ),
    spec(
        "client",
        -2,
        ADMIN | NOSCRIPT | LOADING | STALE,
        NOKEYS,
        Family::Conn,
    ),
    spec("time", 1, FAST | LOADING | STALE, NOKEYS, Family::Server),
    spec("info", -1, LOADING | STALE, NOKEYS, Family::Server),
    spec(
        "config",
        -2,
        ADMIN | NOSCRIPT | LOADING | STALE,
        NOKEYS,
        Family::Server,
    ),
    spec(
        "debug",
        -2,
        ADMIN | NOSCRIPT | LOADING | STALE,
        NOKEYS,
        Family::Server,
    ),
    spec("command", -1, LOADING | STALE, NOKEYS, Family::Server),
    spec("function", -2, NOSCRIPT, NOKEYS, Family::Server),
    spec("script", -2, NOSCRIPT, NOKEYS, Family::Server),
    spec("flushall", -1, WRITE, NOKEYS, Family::Server),
    spec("flushdb", -1, WRITE, NOKEYS, Family::Server),
    spec(
        "lastsave",
        1,
        FAST | LOADING | STALE,
        NOKEYS,
        Family::Server,
    ),
    spec(
        "readwrite",
        1,
        FAST | LOADING | STALE,
        NOKEYS,
        Family::Server,
    ),
    spec(
        "readonly",
        1,
        FAST | LOADING | STALE,
        NOKEYS,
        Family::Server,
    ),
];

/// Commands Loams does not serve in v1 (design §77 §8.5), answered with a
/// specific error instead of "unknown command".
pub static NOT_SUPPORTED: &[&str] = &[
    "eval",
    "evalsha",
    "eval_ro",
    "evalsha_ro",
    "fcall",
    "fcall_ro",
    "subscribe",
    "unsubscribe",
    "psubscribe",
    "punsubscribe",
    "ssubscribe",
    "sunsubscribe",
    "publish",
    "spublish",
    "pubsub",
    "xadd",
    "xrange",
    "xrevrange",
    "xlen",
    "xread",
    "xreadgroup",
    "xgroup",
    "xack",
    "xclaim",
    "xautoclaim",
    "xdel",
    "xtrim",
    "xinfo",
    "xpending",
    "xsetid",
    "blpop",
    "brpop",
    "brpoplpush",
    "blmove",
    "blmpop",
    "bzpopmin",
    "bzpopmax",
    "bzmpop",
    "cluster",
    "asking",
    "monitor",
    "sync",
    "psync",
    "replicaof",
    "slaveof",
    "swapdb",
    "wait",
    "waitaof",
    "bitfield",
    "bitfield_ro",
    "pfadd",
    "pfcount",
    "pfmerge",
    "pfdebug",
    "pfselftest",
    "geoadd",
    "geodist",
    "geohash",
    "geopos",
    "georadius",
    "georadius_ro",
    "georadiusbymember",
    "georadiusbymember_ro",
    "geosearch",
    "geosearchstore",
    "module",
    "save",
    "bgsave",
    "bgrewriteaof",
    "shutdown",
    "failover",
    "migrate",
    "restore",
    "restore-asking",
    "dump",
    "acl",
    "latency",
    "memory",
    "slowlog",
    "lolwut",
];

static BY_NAME: LazyLock<HashMap<&'static str, &'static Spec>> =
    LazyLock::new(|| COMMANDS.iter().map(|s| (s.name, s)).collect());

/// The spec of `name` (any case).
pub fn lookup(name: &[u8]) -> Option<&'static Spec> {
    let lower = String::from_utf8_lossy(name).to_ascii_lowercase();
    BY_NAME.get(lower.as_str()).copied()
}

/// Runs one request and returns its reply.
pub async fn dispatch(shared: &Arc<Shared>, conn: &mut Conn, args: Vec<Bytes>) -> (Value, Flow) {
    shared
        .stats
        .commands
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let Some(name) = args.first() else {
        return (Value::ok(), Flow::Continue);
    };
    let Some(spec) = lookup(name) else {
        let lower = String::from_utf8_lossy(name).to_ascii_lowercase();
        if NOT_SUPPORTED.contains(&lower.as_str()) {
            return (
                Value::err(format!("ERR '{lower}' is not supported by Loams")),
                Flow::Continue,
            );
        }
        return (unknown_command(&args), Flow::Continue);
    };
    if !conn.authenticated && spec.flags & NOAUTH == 0 {
        return (
            Value::err("NOAUTH Authentication required."),
            Flow::Continue,
        );
    }
    if !arity_ok(spec.arity, args.len()) {
        return (wrong_arity(spec.name), Flow::Continue);
    }
    shared.with_client(conn.id, |c| c.last_command = Some(spec.name.to_owned()));
    match spec.family {
        Family::Conn => conn::run(shared, conn, spec, &args),
        Family::Server => (server::run(shared, conn, spec, &args).await, Flow::Continue),
    }
}

fn arity_ok(arity: i32, argc: usize) -> bool {
    let argc = i64::try_from(argc).unwrap_or(i64::MAX);
    let arity = i64::from(arity);
    if arity >= 0 {
        argc == arity
    } else {
        argc >= -arity
    }
}

/// Valkey's unknown-command error: the name and the first arguments, each
/// cut to 128 bytes.
pub fn unknown_command(args: &[Bytes]) -> Value {
    let cut = |b: &Bytes| {
        let s = String::from_utf8_lossy(&b[..b.len().min(128)]).into_owned();
        s.replace(['\r', '\n'], " ")
    };
    let mut text = format!(
        "ERR unknown command '{}', with args beginning with: ",
        cut(&args[0])
    );
    for a in args.iter().skip(1) {
        let piece = format!("'{}' ", cut(a));
        if text.len() + piece.len() > 128 + 64 {
            break;
        }
        text.push_str(&piece);
    }
    Value::err(text)
}

/// Valkey's arity error.
pub fn wrong_arity(name: &str) -> Value {
    Value::err(format!(
        "ERR wrong number of arguments for '{name}' command"
    ))
}

/// Valkey's unknown-subcommand error.
pub fn unknown_subcommand(command: &str, sub: &[u8]) -> Value {
    Value::err(format!(
        "ERR unknown subcommand '{}'. Try {} HELP.",
        String::from_utf8_lossy(&sub[..sub.len().min(128)]),
        command.to_ascii_uppercase()
    ))
}

/// The syntax error reply.
pub fn syntax_error() -> Value {
    Value::err("ERR syntax error")
}

/// The not-an-integer reply.
pub fn not_integer() -> Value {
    Value::err("ERR value is not an integer or out of range")
}

/// An argument as an integer, or Valkey's error.
pub fn int_arg(arg: &[u8]) -> Result<i64, Value> {
    parse_int(arg).ok_or_else(not_integer)
}

/// An argument, upper-cased, for keyword matching.
pub fn upper(arg: &[u8]) -> String {
    String::from_utf8_lossy(arg).to_ascii_uppercase()
}

/// The `COMMAND INFO` entry of a spec.
pub fn command_info(spec: &Spec) -> Value {
    let flags = flag::NAMES
        .iter()
        .filter(|(bit, _)| spec.flags & bit != 0)
        .map(|(_, name)| Value::Simple((*name).into()))
        .collect();
    Value::Array(vec![
        Value::bulk(spec.name),
        Value::Integer(spec.arity.into()),
        Value::Set(flags),
        Value::Integer(spec.first_key.into()),
        Value::Integer(spec.last_key.into()),
        Value::Integer(spec.step.into()),
        Value::Set(vec![]),
        Value::Set(vec![]),
        Value::Array(vec![]),
        Value::Array(vec![]),
    ])
}

/// The key arguments of a request, by its spec's positions.
pub fn keys_of<'a>(spec: &Spec, args: &'a [Bytes]) -> Vec<&'a Bytes> {
    if spec.first_key <= 0 {
        return vec![];
    }
    let n = i32::try_from(args.len()).unwrap_or(i32::MAX);
    let last = if spec.last_key < 0 {
        n + spec.last_key
    } else {
        spec.last_key.min(n - 1)
    };
    let step = usize::try_from(spec.step.max(1)).unwrap_or(1);
    (spec.first_key..=last)
        .step_by(step)
        .filter_map(|i| args.get(usize::try_from(i).ok()?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arity_rules() {
        assert!(arity_ok(2, 2));
        assert!(!arity_ok(2, 3));
        assert!(arity_ok(-2, 2));
        assert!(arity_ok(-2, 5));
        assert!(!arity_ok(-2, 1));
    }

    #[test]
    fn table_names_are_lower_case_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for s in COMMANDS {
            assert_eq!(s.name, s.name.to_ascii_lowercase());
            assert!(seen.insert(s.name), "{} twice", s.name);
            assert!(
                !NOT_SUPPORTED.contains(&s.name),
                "{} is served and refused",
                s.name
            );
        }
    }

    #[test]
    fn unknown_command_text() {
        let args = vec![Bytes::from_static(b"foo"), Bytes::from_static(b"a")];
        assert_eq!(
            unknown_command(&args),
            Value::err("ERR unknown command 'foo', with args beginning with: 'a' ")
        );
    }
}
