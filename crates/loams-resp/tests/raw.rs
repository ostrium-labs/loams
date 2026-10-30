//! Raw databases over TCP (coordinator ruling 2026-10-30, `cmd/raw.rs`): a
//! database in `raw_databases` serves strings on the raw store and refuses
//! the rest; the others are unchanged. The memory store's raw databases live
//! in memory; `loams-kv`'s `tests/raw.rs` runs the raw store itself on TiKV.

use loams_resp::Value;
use loams_resp::testing::{Client, TestServer, config, start};

async fn server() -> TestServer {
    let store = loams_kv::testing::embedded().await;
    start(config("raw_databases = [1]"), store).await
}

async fn raw_client(s: &TestServer) -> Client {
    let mut c = Client::connect(s.addr).await;
    assert_eq!(c.call(&["SELECT", "1"]).await, Value::ok());
    c
}

fn int(n: i64) -> Value {
    Value::Integer(n)
}

fn bulk(s: &str) -> Value {
    Value::bulk(s.to_owned())
}

#[tokio::test]
async fn strings_round_trip() {
    let s = server().await;
    let mut c = raw_client(&s).await;
    assert_eq!(c.call(&["GET", "a"]).await, Value::Null);
    assert_eq!(c.call(&["SET", "a", "1"]).await, Value::ok());
    assert_eq!(c.call(&["GET", "a"]).await, bulk("1"));
    assert_eq!(c.call(&["STRLEN", "a"]).await, int(1));
    assert_eq!(c.call(&["TYPE", "a"]).await, Value::Simple("string".into()));
    assert_eq!(c.call(&["MSET", "b", "2", "c", "3"]).await, Value::ok());
    assert_eq!(
        c.call(&["MGET", "a", "x", "c"]).await,
        Value::Array(vec![bulk("1"), Value::Null, bulk("3")])
    );
    assert_eq!(c.call(&["EXISTS", "a", "a", "x"]).await, int(2));
    assert_eq!(c.call(&["DEL", "a", "b", "x"]).await, int(2));
    assert_eq!(c.call(&["GET", "a"]).await, Value::Null);
}

#[tokio::test]
async fn conditional_sets_and_counters() {
    let s = server().await;
    let mut c = raw_client(&s).await;
    assert_eq!(c.call(&["SETNX", "k", "1"]).await, int(1));
    assert_eq!(c.call(&["SETNX", "k", "2"]).await, int(0));
    assert_eq!(c.call(&["SET", "k", "3", "NX"]).await, Value::Null);
    assert_eq!(c.call(&["SET", "k", "4", "XX", "GET"]).await, bulk("1"));
    assert_eq!(c.call(&["SET", "nope", "4", "XX"]).await, Value::Null);
    assert_eq!(c.call(&["GETSET", "k", "5"]).await, bulk("4"));
    assert_eq!(c.call(&["INCR", "n"]).await, int(1));
    assert_eq!(c.call(&["INCRBY", "n", "10"]).await, int(11));
    assert_eq!(c.call(&["DECRBY", "n", "15"]).await, int(-4));
    assert_eq!(
        c.call(&["INCR", "k"]).await,
        int(6),
        "a decimal string counts"
    );
    assert_eq!(c.call(&["SET", "s", "abc"]).await, Value::ok());
    assert_eq!(
        c.call(&["INCR", "s"]).await,
        Value::err("ERR value is not an integer or out of range")
    );
}

#[tokio::test]
async fn ttls_and_their_limits() {
    let s = server().await;
    let mut c = raw_client(&s).await;
    assert_eq!(c.call(&["TTL", "none"]).await, int(-2));
    assert_eq!(c.call(&["SET", "p", "v"]).await, Value::ok());
    assert_eq!(c.call(&["TTL", "p"]).await, int(-1));
    assert_eq!(c.call(&["SET", "t", "v", "EX", "100"]).await, Value::ok());
    let Value::Integer(left) = c.call(&["TTL", "t"]).await else {
        panic!("TTL is an integer");
    };
    assert!((99..=100).contains(&left), "{left}");
    assert_eq!(c.call(&["SETEX", "u", "100", "1"]).await, Value::ok());
    // What TiKV's raw API cannot do is refused, not done racily.
    for refused in [
        &["INCR", "u"][..],
        &["SET", "t", "v", "NX", "EX", "5"][..],
        &["SET", "t", "v", "KEEPTTL"][..],
        &["EXPIRE", "p", "5"][..],
        &["PERSIST", "t"][..],
    ] {
        let reply = c.call(refused).await;
        assert!(
            matches!(&reply, Value::Error(e) if e.contains("raw database")),
            "{refused:?}: {reply:?}"
        );
    }
    // A past absolute expiry keeps nothing.
    assert_eq!(
        c.call(&["SET", "gone", "v", "PXAT", "1"]).await,
        Value::ok()
    );
    assert_eq!(c.call(&["GET", "gone"]).await, Value::Null);
}

#[tokio::test]
async fn other_types_are_refused_and_other_databases_unchanged() {
    let s = server().await;
    let mut c = raw_client(&s).await;
    for cmd in [
        &["HSET", "h", "f", "v"][..],
        &["SADD", "s", "m"][..],
        &["ZADD", "z", "1", "m"][..],
        &["LPUSH", "l", "x"][..],
        &["KEYS", "*"][..],
        &["FLUSHDB"][..],
    ] {
        let reply = c.call(cmd).await;
        assert!(
            matches!(&reply, Value::Error(e) if e.contains("raw database 1")),
            "{cmd:?}: {reply:?}"
        );
    }
    assert!(matches!(c.call(&["FLUSHALL"]).await, Value::Error(e) if e.contains("raw_databases")));
    // Connection and server commands still work in a raw database.
    assert_eq!(c.call(&["PING"]).await, Value::Simple("PONG".into()));
    // Database 0 is transactional: hashes work there, and its keys are not
    // database 1's.
    assert_eq!(c.call(&["SET", "k", "raw"]).await, Value::ok());
    assert_eq!(c.call(&["SELECT", "0"]).await, Value::ok());
    assert_eq!(c.call(&["GET", "k"]).await, Value::Null);
    assert_eq!(c.call(&["HSET", "h", "f", "v"]).await, int(1));
}

#[test]
fn raw_databases_are_checked() {
    let bad = loams_resp::RespConfig::from_toml(
        "listen = \"127.0.0.1:0\"\ndatabases = 4\nraw_databases = [4]\n\
         [store]\nkind = \"memory\"\nname = \"t\"\n",
    );
    assert!(bad.unwrap_err().contains("raw_databases names database 4"));
    let embedded = loams_resp::RespConfig::from_toml(
        "listen = \"127.0.0.1:0\"\nraw_databases = [1]\n\
         [store]\nkind = \"embedded\"\npath = \"/nonexistent/x.redb\"\nkeyspace = \"kv\"\n",
    );
    assert!(embedded.unwrap_err().contains("needs the tikv store"));
}
