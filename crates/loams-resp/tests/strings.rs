//! Strings and the generic key commands over TCP (plan KV2 Task 3), on
//! every backend `loams_kv::testing::stores` gives (embedded always, TiKV
//! where `LOAMS_TEST_PD` is set).

use std::time::Duration;

use loams_kv::Store;
use loams_resp::Value;
use loams_resp::layout::{self, Kind, Meta};
use loams_resp::testing::{Client, TestServer, config, start};

async fn servers(name: &str) -> Vec<(Store, TestServer)> {
    let mut out = Vec::new();
    for store in loams_kv::testing::stores(name).await {
        let server = start(config(""), store.clone()).await;
        out.push((store, server));
    }
    out
}

fn bulk(s: &str) -> Value {
    Value::bulk(s.to_owned())
}

fn int(n: i64) -> Value {
    Value::Integer(n)
}

const WRONGTYPE: &str = "WRONGTYPE Operation against a key holding the wrong kind of value";

/// Plants a key of another type (a hash's meta record, no fields), as the
/// hash commands (Task 5) will write it.
async fn plant_hash(store: &Store, db: u8, key: &str) {
    let meta = Meta {
        kind: Kind::Hash,
        ver: 1,
        expire_at_ms: 0,
        trailer: vec![],
    };
    store
        .put(&layout::meta_key(db, key.as_bytes()), meta.encode())
        .await
        .expect("a put");
}

#[tokio::test]
async fn set_and_get_basics() {
    for (_, s) in servers("set_and_get_basics").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["GET", "k"]).await, Value::Null);
        assert_eq!(c.call(&["SET", "k", "v1"]).await, Value::ok());
        assert_eq!(c.call(&["GET", "k"]).await, bulk("v1"));
        assert_eq!(c.call(&["SET", "k", "v2", "NX"]).await, Value::Null);
        assert_eq!(c.call(&["SET", "k", "v2", "XX"]).await, Value::ok());
        assert_eq!(c.call(&["SET", "other", "x", "XX"]).await, Value::Null);
        assert_eq!(c.call(&["SET", "k", "v3", "GET"]).await, bulk("v2"));
        assert_eq!(c.call(&["SET", "new", "n", "NX", "GET"]).await, Value::Null);
        assert_eq!(c.call(&["GET", "new"]).await, bulk("n"));
        assert_eq!(c.call(&["SETNX", "new", "again"]).await, int(0));
        assert_eq!(c.call(&["SETNX", "fresh", "f"]).await, int(1));
        assert_eq!(c.call(&["GETSET", "k", "v4"]).await, bulk("v3"));
        assert_eq!(c.call(&["GETDEL", "k"]).await, bulk("v4"));
        assert_eq!(c.call(&["GET", "k"]).await, Value::Null);
        assert_eq!(
            c.call(&["SET", "k", "v", "NX", "XX"]).await,
            Value::err("ERR syntax error")
        );
        assert_eq!(
            c.call(&["SET", "k", "v", "EX", "0"]).await,
            Value::err("ERR invalid expire time in 'set' command")
        );
        assert_eq!(
            c.call(&["SETEX", "k", "-1", "v"]).await,
            Value::err("ERR invalid expire time in 'setex' command")
        );
        // Binary keys and values.
        c.send(&[b"SET", b"\x00\xffkey", b"\x00\r\nv"]).await;
        assert_eq!(c.read().await, Some(Value::ok()));
        c.send(&[b"GET", b"\x00\xffkey"]).await;
        assert_eq!(c.read().await, Some(Value::bulk(&b"\x00\r\nv"[..])));
    }
}

#[tokio::test]
async fn set_expiry_options() {
    for (_, s) in servers("set_expiry_options").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["SET", "k", "v", "EX", "100"]).await, Value::ok());
        let Value::Integer(ttl) = c.call(&["TTL", "k"]).await else {
            panic!("a TTL");
        };
        assert!((99..=100).contains(&ttl), "{ttl}");
        // KEEPTTL keeps it; a plain SET clears it.
        assert_eq!(c.call(&["SET", "k", "w", "KEEPTTL"]).await, Value::ok());
        assert!(matches!(c.call(&["PTTL", "k"]).await, Value::Integer(n) if n > 90_000));
        assert_eq!(c.call(&["SET", "k", "w"]).await, Value::ok());
        assert_eq!(c.call(&["TTL", "k"]).await, int(-1));
        // PXAT in the past: set, then gone at once.
        assert_eq!(c.call(&["SET", "k", "w", "PXAT", "1"]).await, Value::ok());
        assert_eq!(c.call(&["EXISTS", "k"]).await, int(0));
        // EXAT in the future.
        assert_eq!(
            c.call(&["SET", "k", "w", "EXAT", "4102444800"]).await,
            Value::ok()
        );
        assert_eq!(c.call(&["EXPIRETIME", "k"]).await, int(4_102_444_800));
        assert_eq!(c.call(&["PEXPIRETIME", "k"]).await, int(4_102_444_800_000));
        assert_eq!(c.call(&["PERSIST", "k"]).await, int(1));
        assert_eq!(c.call(&["PERSIST", "k"]).await, int(0));
        assert_eq!(c.call(&["TTL", "missing"]).await, int(-2));
    }
}

#[tokio::test]
async fn lazy_expiry_hides_and_replaces() {
    for (_, s) in servers("lazy_expiry_hides_and_replaces").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["SET", "k", "v", "PX", "50"]).await, Value::ok());
        assert_eq!(c.call(&["PSETEX", "p", "50", "v"]).await, Value::ok());
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(c.call(&["GET", "k"]).await, Value::Null);
        assert_eq!(c.call(&["TTL", "k"]).await, int(-2));
        assert_eq!(c.call(&["EXISTS", "k", "p"]).await, int(0));
        assert_eq!(c.call(&["DBSIZE"]).await, int(0));
        // A write over the expired key starts afresh, with no TTL.
        assert_eq!(c.call(&["APPEND", "k", "new"]).await, int(3));
        assert_eq!(c.call(&["GET", "k"]).await, bulk("new"));
        assert_eq!(c.call(&["TTL", "k"]).await, int(-1));
        assert_eq!(c.call(&["SET", "p", "x", "NX"]).await, Value::ok());
    }
}

#[tokio::test]
async fn wrongtype_on_every_write() {
    for (store, s) in servers("wrongtype_on_every_write").await {
        plant_hash(&store, 0, "h").await;
        let mut c = Client::connect(s.addr).await;
        let wrong = Value::err(WRONGTYPE);
        for cmd in [
            &["GET", "h"][..],
            &["GETSET", "h", "v"],
            &["GETDEL", "h"],
            &["GETEX", "h", "PERSIST"],
            &["APPEND", "h", "v"],
            &["STRLEN", "h"],
            &["GETRANGE", "h", "0", "-1"],
            &["SETRANGE", "h", "0", "v"],
            &["SET", "h", "v", "GET"],
        ] {
            assert_eq!(c.call(cmd).await, wrong, "{cmd:?}");
        }
        // Still a hash after all of them.
        assert_eq!(c.call(&["TYPE", "h"]).await, Value::Simple("hash".into()));
        // MGET answers the null for another type.
        assert_eq!(
            c.call(&["MGET", "h", "none"]).await,
            Value::Array(vec![Value::Null, Value::Null])
        );
        // SET replaces any type, and the hash's version is queued for GC.
        assert_eq!(c.call(&["SET", "h", "v"]).await, Value::ok());
        assert_eq!(c.call(&["TYPE", "h"]).await, Value::Simple("string".into()));
        assert!(
            store
                .get(&layout::gc_key(0, b"h", 1))
                .await
                .expect("a get")
                .is_some(),
            "the hash's version is in the GC queue"
        );
    }
}

#[tokio::test]
async fn multi_key_and_range_commands() {
    for (_, s) in servers("multi_key_and_range_commands").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["MSET", "a", "1", "b", "2"]).await, Value::ok());
        assert_eq!(
            c.call(&["MSET", "a", "1", "b"]).await,
            Value::err("ERR wrong number of arguments for 'mset' command")
        );
        assert_eq!(
            c.call(&["MGET", "a", "x", "b"]).await,
            Value::Array(vec![bulk("1"), Value::Null, bulk("2")])
        );
        assert_eq!(c.call(&["MSETNX", "a", "9", "c", "3"]).await, int(0));
        assert_eq!(c.call(&["GET", "c"]).await, Value::Null);
        assert_eq!(c.call(&["MSETNX", "c", "3", "d", "4"]).await, int(1));
        assert_eq!(c.call(&["SET", "s", "Hello World"]).await, Value::ok());
        assert_eq!(c.call(&["STRLEN", "s"]).await, int(11));
        assert_eq!(c.call(&["STRLEN", "none"]).await, int(0));
        assert_eq!(c.call(&["GETRANGE", "s", "0", "4"]).await, bulk("Hello"));
        assert_eq!(c.call(&["GETRANGE", "s", "-5", "-1"]).await, bulk("World"));
        assert_eq!(c.call(&["SUBSTR", "s", "5", "3"]).await, bulk(""));
        assert_eq!(c.call(&["SETRANGE", "s", "6", "Redis"]).await, int(11));
        assert_eq!(c.call(&["GET", "s"]).await, bulk("Hello Redis"));
        assert_eq!(c.call(&["SETRANGE", "pad", "3", "x"]).await, int(4));
        assert_eq!(
            c.call(&["GET", "pad"]).await,
            Value::bulk(&b"\x00\x00\x00x"[..])
        );
        assert_eq!(c.call(&["SETRANGE", "nothing", "5", ""]).await, int(0));
        assert_eq!(c.call(&["EXISTS", "nothing"]).await, int(0));
        assert_eq!(
            c.call(&["SETRANGE", "s", "-1", "x"]).await,
            Value::err("ERR offset is out of range")
        );
        assert_eq!(
            c.call(&["SETRANGE", "s", "536870911", "xy"]).await,
            Value::err("ERR string exceeds maximum allowed size (proto-max-bulk-len)")
        );
        assert_eq!(c.call(&["APPEND", "s", "!"]).await, int(12));
        assert_eq!(c.call(&["EXISTS", "a", "a", "zz"]).await, int(2));
        assert_eq!(c.call(&["TOUCH", "a", "b", "zz"]).await, int(2));
        assert_eq!(c.call(&["TYPE", "zz"]).await, Value::Simple("none".into()));
        assert_eq!(c.call(&["DBSIZE"]).await, int(6));
        assert_eq!(c.call(&["DEL", "a", "b", "zz"]).await, int(2));
        assert_eq!(c.call(&["UNLINK", "c"]).await, int(1));
        assert_eq!(c.call(&["DBSIZE"]).await, int(3));
        assert_eq!(
            c.call(&["LCS", "s", "pad"]).await,
            Value::err("ERR 'lcs' is not supported by Loams")
        );
    }
}

#[tokio::test]
async fn expire_family() {
    for (_, s) in servers("expire_family").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["EXPIRE", "missing", "10"]).await, int(0));
        assert_eq!(c.call(&["SET", "k", "v"]).await, Value::ok());
        assert_eq!(c.call(&["EXPIRE", "k", "100", "XX"]).await, int(0));
        assert_eq!(c.call(&["EXPIRE", "k", "100", "GT"]).await, int(0));
        assert_eq!(c.call(&["EXPIRE", "k", "100", "NX"]).await, int(1));
        assert_eq!(c.call(&["EXPIRE", "k", "200", "NX"]).await, int(0));
        assert_eq!(c.call(&["EXPIRE", "k", "50", "GT"]).await, int(0));
        assert_eq!(c.call(&["EXPIRE", "k", "200", "GT"]).await, int(1));
        assert_eq!(c.call(&["EXPIRE", "k", "300", "LT"]).await, int(0));
        assert_eq!(c.call(&["PEXPIRE", "k", "150000", "LT"]).await, int(1));
        assert!(
            matches!(c.call(&["TTL", "k"]).await, Value::Integer(n) if (149..=150).contains(&n))
        );
        assert_eq!(
            c.call(&["EXPIRE", "k", "1", "NX", "XX"]).await,
            Value::err("ERR NX and XX, GT or LT options at the same time are not compatible")
        );
        assert_eq!(
            c.call(&["EXPIRE", "k", "1", "GT", "LT"]).await,
            Value::err("ERR GT and LT options at the same time are not compatible")
        );
        assert_eq!(
            c.call(&["EXPIRE", "k", "1", "BOGUS"]).await,
            Value::err("ERR Unsupported option BOGUS")
        );
        assert_eq!(
            c.call(&["EXPIRE", "k", "9223372036854775807"]).await,
            Value::err("ERR invalid expire time in 'expire' command")
        );
        assert_eq!(
            c.call(&["EXPIRE", "k", "x"]).await,
            Value::err("ERR value is not an integer or out of range")
        );
        // A time in the past deletes the key.
        assert_eq!(c.call(&["EXPIREAT", "k", "1"]).await, int(1));
        assert_eq!(c.call(&["EXISTS", "k"]).await, int(0));
        assert_eq!(c.call(&["SET", "k", "v"]).await, Value::ok());
        assert_eq!(c.call(&["PEXPIREAT", "k", "-5"]).await, int(1));
        assert_eq!(c.call(&["GET", "k"]).await, Value::Null);
        // GETEX sets and clears TTLs.
        assert_eq!(c.call(&["SET", "g", "v"]).await, Value::ok());
        assert_eq!(c.call(&["GETEX", "g", "EX", "100"]).await, bulk("v"));
        assert!(matches!(c.call(&["TTL", "g"]).await, Value::Integer(n) if n > 90));
        assert_eq!(c.call(&["GETEX", "g", "PERSIST"]).await, bulk("v"));
        assert_eq!(c.call(&["TTL", "g"]).await, int(-1));
        assert_eq!(
            c.call(&["GETEX", "g", "EX", "1", "PERSIST"]).await,
            Value::err("ERR syntax error")
        );
        assert_eq!(c.call(&["GETEX", "nope"]).await, Value::Null);
    }
}

#[tokio::test]
async fn large_string_chunks_roundtrip() {
    for (_, s) in servers("large_string_chunks_roundtrip").await {
        let mut c = Client::connect(s.addr).await;
        let value: Vec<u8> = (0..(3 << 20) + 123)
            .map(|i: usize| u8::try_from(i % 253).unwrap_or(0))
            .collect();
        c.send(&[b"SET", b"big", &value]).await;
        assert_eq!(c.read().await, Some(Value::ok()));
        c.send(&[b"GET", b"big"]).await;
        assert_eq!(c.read().await, Some(Value::bulk(value.clone())));
        assert_eq!(
            c.call(&["STRLEN", "big"]).await,
            int(i64::try_from(value.len()).unwrap_or(0))
        );
        let (from, to) = ((1 << 20) - 2, (1 << 20) + 2);
        assert_eq!(
            c.call(&["GETRANGE", "big", &from.to_string(), &to.to_string()])
                .await,
            Value::bulk(value[from..=to].to_vec())
        );
        assert_eq!(
            c.call(&["APPEND", "big", "!"]).await,
            int(i64::try_from(value.len() + 1).unwrap_or(0))
        );
        assert_eq!(c.call(&["SET", "big", "small"]).await, Value::ok());
        assert_eq!(c.call(&["GET", "big"]).await, bulk("small"));
        assert_eq!(c.call(&["DEL", "big"]).await, int(1));
    }
}

/// Databases are separate: a key set in one is absent in another, and
/// `FLUSHDB` clears only the selected one.
#[tokio::test]
async fn flushdb_spares_other_dbs() {
    for (_, s) in servers("strings_flushdb_spares_other_dbs").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["SET", "k", "zero"]).await, Value::ok());
        assert_eq!(c.call(&["SELECT", "1"]).await, Value::ok());
        assert_eq!(c.call(&["GET", "k"]).await, Value::Null);
        assert_eq!(c.call(&["SET", "k", "one", "EX", "100"]).await, Value::ok());
        assert_eq!(c.call(&["FLUSHDB"]).await, Value::ok());
        assert_eq!(c.call(&["DBSIZE"]).await, int(0));
        assert_eq!(c.call(&["SELECT", "0"]).await, Value::ok());
        assert_eq!(c.call(&["GET", "k"]).await, bulk("zero"));
    }
}

/// Sixteen clients each set their own keys at once; every write lands.
#[tokio::test]
async fn concurrent_sets_all_land() {
    for (_, s) in servers("concurrent_sets_all_land").await {
        let mut tasks = Vec::new();
        for n in 0..16 {
            let addr = s.addr;
            tasks.push(tokio::spawn(async move {
                let mut c = Client::connect(addr).await;
                for i in 0..20 {
                    let key = format!("c{n}:{i}");
                    assert_eq!(c.call(&["SET", &key, "v"]).await, Value::ok());
                }
            }));
        }
        for t in tasks {
            t.await.expect("a client task");
        }
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["DBSIZE"]).await, int(320));
    }
}
