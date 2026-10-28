//! Connection commands: `PING`, `ECHO`, `HELLO`, `AUTH`, `SELECT`,
//! `RESET`, `QUIT`, `CLIENT` (plan KV2 Task 2). Replies and error texts
//! follow Valkey's documented behaviour.

use std::sync::Arc;

use bytes::Bytes;

use super::{Flow, Spec, int_arg, syntax_error, unknown_subcommand, upper, wrong_arity};
use crate::frame::{Proto, Value};
use crate::server::{Conn, ReplyMode, Shared, client_line, verbatim};

/// The version reported to clients: the Redis protocol level Loams aims at.
pub const REDIS_VERSION: &str = "7.2.4";

pub(super) fn run(
    shared: &Arc<Shared>,
    conn: &mut Conn,
    spec: &Spec,
    args: &[Bytes],
) -> (Value, Flow) {
    let reply = match spec.name {
        "ping" => match args.len() {
            1 => Value::Simple("PONG".into()),
            2 => Value::Bulk(args[1].clone()),
            _ => wrong_arity("ping"),
        },
        "echo" => Value::Bulk(args[1].clone()),
        "hello" => hello(shared, conn, args),
        "auth" => auth(shared, conn, args),
        "select" => select(shared, conn, &args[1]),
        "reset" => {
            reset(shared, conn);
            Value::Simple("RESET".into())
        }
        "quit" => return (Value::ok(), Flow::Close),
        "client" => client(shared, conn, args),
        other => Value::err(format!("ERR '{other}' has no handler")),
    };
    (reply, Flow::Continue)
}

fn check_password(shared: &Shared, user: &[u8], pass: &[u8]) -> Result<(), Value> {
    match &shared.password {
        None if user == b"default" => Ok(()),
        Some(p) if user == b"default" && p.as_bytes() == pass => Ok(()),
        _ => Err(Value::err(
            "WRONGPASS invalid username-password pair or user is disabled.",
        )),
    }
}

fn auth(shared: &Shared, conn: &mut Conn, args: &[Bytes]) -> Value {
    let (user, pass): (&[u8], &[u8]) = match args.len() {
        2 => (b"default", &args[1]),
        3 => (&args[1], &args[2]),
        _ => return syntax_error(),
    };
    if args.len() == 2 && shared.password.is_none() {
        return Value::err(
            "ERR AUTH <password> called without any password configured for the default user. Are you sure your configuration is correct?",
        );
    }
    match check_password(shared, user, pass) {
        Ok(()) => {
            conn.authenticated = true;
            Value::ok()
        }
        Err(e) => e,
    }
}

fn hello(shared: &Shared, conn: &mut Conn, args: &[Bytes]) -> Value {
    let mut proto = conn.proto;
    let mut i = 1;
    if let Some(ver) = args.get(1) {
        match int_arg(ver) {
            Ok(2) => proto = Proto::Resp2,
            Ok(3) => proto = Proto::Resp3,
            Ok(_) => return Value::err("NOPROTO unsupported protocol version"),
            Err(_) => {
                return Value::err("ERR Protocol version is not an integer or out of range");
            }
        }
        i = 2;
    }
    let mut name = None;
    while i < args.len() {
        match upper(&args[i]).as_str() {
            "AUTH" if i + 2 < args.len() => {
                if let Err(e) = check_password(shared, &args[i + 1], &args[i + 2]) {
                    return e;
                }
                conn.authenticated = true;
                i += 3;
            }
            "SETNAME" if i + 1 < args.len() => {
                if let Err(e) = valid_name(&args[i + 1]) {
                    return e;
                }
                name = Some(args[i + 1].clone());
                i += 2;
            }
            _ => {
                return Value::err(format!(
                    "ERR Syntax error in HELLO option '{}'",
                    String::from_utf8_lossy(&args[i])
                ));
            }
        }
    }
    if !conn.authenticated {
        return Value::err(
            "NOAUTH HELLO must be called with the client already authenticated, otherwise the HELLO <proto> AUTH <user> <pass> option can be used to authenticate the client and select the RESP protocol version at the same time",
        );
    }
    if let Some(name) = name {
        set_name(shared, conn, Some(name));
    }
    conn.proto = proto;
    let version = match proto {
        Proto::Resp2 => 2,
        Proto::Resp3 => 3,
    };
    Value::Map(vec![
        (Value::bulk("server"), Value::bulk("loams")),
        (Value::bulk("version"), Value::bulk(REDIS_VERSION)),
        (Value::bulk("proto"), Value::Integer(version)),
        (
            Value::bulk("id"),
            Value::Integer(i64::try_from(conn.id).unwrap_or(i64::MAX)),
        ),
        (Value::bulk("mode"), Value::bulk("standalone")),
        (Value::bulk("role"), Value::bulk("master")),
        (Value::bulk("modules"), Value::Array(vec![])),
    ])
}

fn select(shared: &Shared, conn: &mut Conn, arg: &[u8]) -> Value {
    let db = match int_arg(arg) {
        Ok(db) => db,
        Err(e) => return e,
    };
    if db < 0 || db >= i64::from(shared.databases) {
        return Value::err("ERR DB index is out of range");
    }
    conn.db = u8::try_from(db).unwrap_or(0);
    let db = conn.db;
    shared.with_client(conn.id, |c| c.db = db);
    Value::ok()
}

/// `RESET`: the connection's state back to a new connection's.
pub fn reset(shared: &Shared, conn: &mut Conn) {
    conn.proto = Proto::Resp2;
    conn.db = 0;
    conn.reply = ReplyMode::On;
    conn.authenticated = shared.password.is_none();
    set_name(shared, conn, None);
    shared.with_client(conn.id, |c| c.db = 0);
}

fn valid_name(name: &[u8]) -> Result<(), Value> {
    if name.iter().any(|b| *b < b'!' || *b > b'~') {
        return Err(Value::err(
            "ERR Client names cannot contain spaces, newlines or special characters.",
        ));
    }
    Ok(())
}

fn set_name(shared: &Shared, conn: &mut Conn, name: Option<Bytes>) {
    let name = name.filter(|n| !n.is_empty());
    conn.name.clone_from(&name);
    shared.with_client(conn.id, |c| c.name = name);
}

fn client(shared: &Shared, conn: &mut Conn, args: &[Bytes]) -> Value {
    let sub = upper(&args[1]);
    let n = args.len();
    match (sub.as_str(), n) {
        ("ID", 2) => Value::Integer(i64::try_from(conn.id).unwrap_or(i64::MAX)),
        ("GETNAME", 2) => conn.name.clone().map_or(Value::Null, Value::Bulk),
        ("SETNAME", 3) => match valid_name(&args[2]) {
            Ok(()) => {
                set_name(shared, conn, Some(args[2].clone()));
                Value::ok()
            }
            Err(e) => e,
        },
        ("INFO", 2) => {
            let clients = shared.clients.lock().unwrap_or_else(|e| e.into_inner());
            let line = clients
                .get(&conn.id)
                .map(|c| client_line(conn.id, c))
                .unwrap_or_default();
            verbatim(format!("{line}\n"))
        }
        ("LIST", _) => {
            let clients = shared.clients.lock().unwrap_or_else(|e| e.into_inner());
            let mut text = String::new();
            for (id, c) in clients.iter() {
                text.push_str(&client_line(*id, c));
                text.push('\n');
            }
            verbatim(text)
        }
        ("KILL", _) => kill(shared, conn, &args[2..]),
        ("SETINFO", 4) => {
            let value = String::from_utf8_lossy(&args[3]).into_owned();
            match upper(&args[2]).as_str() {
                "LIB-NAME" => {
                    shared.with_client(conn.id, |c| c.lib_name = Some(value));
                    Value::ok()
                }
                "LIB-VER" => {
                    shared.with_client(conn.id, |c| c.lib_ver = Some(value));
                    Value::ok()
                }
                _ => Value::err(format!(
                    "ERR Unrecognized option '{}'",
                    String::from_utf8_lossy(&args[2])
                )),
            }
        }
        ("NO-EVICT" | "NO-TOUCH", 3) => match upper(&args[2]).as_str() {
            "ON" | "OFF" => Value::ok(),
            _ => syntax_error(),
        },
        ("REPLY", 3) => match upper(&args[2]).as_str() {
            "ON" => {
                conn.reply = ReplyMode::On;
                Value::ok()
            }
            "OFF" => {
                conn.reply = ReplyMode::Off;
                Value::ok()
            }
            "SKIP" => {
                conn.reply = ReplyMode::Skip;
                Value::ok()
            }
            _ => syntax_error(),
        },
        ("TRACKING" | "TRACKINGINFO" | "GETREDIR" | "CACHING", _) => {
            Value::err("ERR client-side caching (CLIENT TRACKING) is not supported by Loams")
        }
        ("PAUSE" | "UNPAUSE" | "UNBLOCK", _) => {
            Value::err(format!("ERR CLIENT {sub} is not supported by Loams"))
        }
        (
            "ID" | "GETNAME" | "SETNAME" | "INFO" | "SETINFO" | "NO-EVICT" | "NO-TOUCH" | "REPLY",
            _,
        ) => Value::err(format!(
            "ERR wrong number of arguments for 'client|{}' command",
            sub.to_ascii_lowercase()
        )),
        _ => unknown_subcommand("client", &args[1]),
    }
}

/// `CLIENT KILL ID <id>`, `CLIENT KILL ADDR <addr>` (with `SKIPME`), or the
/// old `CLIENT KILL <addr>` form.
fn kill(shared: &Shared, conn: &Conn, args: &[Bytes]) -> Value {
    let clients = shared.clients.lock().unwrap_or_else(|e| e.into_inner());
    if args.len() == 1 {
        let addr = String::from_utf8_lossy(&args[0]);
        return match clients.values().find(|c| c.addr.to_string() == addr) {
            Some(c) => {
                let _ = c.kill.send(true);
                Value::ok()
            }
            None => Value::err("ERR No such client"),
        };
    }
    if args.is_empty() || !args.len().is_multiple_of(2) {
        return syntax_error();
    }
    let mut id = None;
    let mut addr = None;
    let mut skipme = true;
    for pair in args.chunks(2) {
        let value = String::from_utf8_lossy(&pair[1]).into_owned();
        match upper(&pair[0]).as_str() {
            "ID" => match value.parse::<u64>() {
                Ok(v) if v > 0 => id = Some(v),
                _ => return Value::err("ERR client-id should be greater than 0"),
            },
            "ADDR" => addr = Some(value),
            "SKIPME" => match value.to_ascii_lowercase().as_str() {
                "yes" => skipme = true,
                "no" => skipme = false,
                _ => return syntax_error(),
            },
            _ => return syntax_error(),
        }
    }
    let mut killed = 0;
    for (cid, c) in clients.iter() {
        if id.is_some_and(|i| i != *cid) || addr.as_ref().is_some_and(|a| *a != c.addr.to_string())
        {
            continue;
        }
        if skipme && *cid == conn.id {
            continue;
        }
        let _ = c.kill.send(true);
        killed += 1;
    }
    Value::Integer(killed)
}
