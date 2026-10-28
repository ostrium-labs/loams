//! Keyspace commands over TCP (plan KV2 Task 9), on every backend.

use std::collections::BTreeSet;

use loams_kv::Store;
use loams_resp::Value;
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

fn names(v: &Value) -> BTreeSet<String> {
    let Value::Array(items) = v else {
        panic!("an array: {v:?}");
    };
    items
        .iter()
        .map(|i| match i {
            Value::Bulk(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("a bulk string: {other:?}"),
        })
        .collect()
}

fn set_of(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// Runs a whole `SCAN` with `extra` options, calling `between` after each
/// page, and returns every key returned.
async fn full_scan(
    c: &mut Client,
    extra: &[&str],
    mut between: impl FnMut(usize) -> Vec<Vec<String>>,
) -> (Vec<String>, Vec<Vec<String>>) {
    let mut seen = Vec::new();
    let mut later = Vec::new();
    let mut cursor = "0".to_owned();
    let mut page = 0;
    loop {
        let mut args = vec!["SCAN".to_owned(), cursor.clone()];
        args.extend(extra.iter().map(|s| (*s).to_owned()));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let Value::Array(reply) = c.call(&refs).await else {
            panic!("a SCAN reply");
        };
        let [Value::Bulk(next), items] = reply.as_slice() else {
            panic!("cursor and items");
        };
        seen.extend(names(items));
        cursor = String::from_utf8_lossy(next).into_owned();
        for cmd in between(page) {
            later.push(cmd);
        }
        page += 1;
        if cursor == "0" {
            return (seen, later);
        }
    }
}

#[tokio::test]
async fn keys_and_scan_filters() {
    for (_, s) in servers("keys_and_scan_filters").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(
            c.call(&["MSET", "user:1", "a", "user:2", "b", "order:1", "c"])
                .await,
            Value::ok()
        );
        assert_eq!(c.call(&["HSET", "user:h", "f", "v"]).await, int(1));
        assert_eq!(
            names(&c.call(&["KEYS", "user:*"]).await),
            set_of(&["user:1", "user:2", "user:h"])
        );
        assert_eq!(
            names(&c.call(&["KEYS", "user:?"]).await),
            set_of(&["user:1", "user:2", "user:h"])
        );
        assert_eq!(names(&c.call(&["KEYS", "*"]).await).len(), 4);
        assert_eq!(names(&c.call(&["KEYS", "nothing*"]).await), set_of(&[]));
        let (all, _) = full_scan(&mut c, &["COUNT", "1"], |_| vec![]).await;
        assert_eq!(all.len(), 4, "{all:?}");
        let (matched, _) = full_scan(&mut c, &["MATCH", "order:*"], |_| vec![]).await;
        assert_eq!(matched, vec!["order:1".to_owned()]);
        let (hashes, _) = full_scan(&mut c, &["TYPE", "hash", "COUNT", "2"], |_| vec![]).await;
        assert_eq!(hashes, vec!["user:h".to_owned()]);
        let (none, _) = full_scan(&mut c, &["TYPE", "nosuchtype"], |_| vec![]).await;
        assert!(none.is_empty());
        assert_eq!(
            c.call(&["SCAN", "x"]).await,
            Value::err("ERR invalid cursor")
        );
        assert_eq!(
            c.call(&["SCAN", "0", "COUNT", "0"]).await,
            Value::err("ERR syntax error")
        );
        // Another database is separate.
        assert_eq!(c.call(&["SELECT", "2"]).await, Value::ok());
        assert_eq!(names(&c.call(&["KEYS", "*"]).await), set_of(&[]));
    }
}

/// Every key present for the whole scan is returned at least once, while
/// other keys are added and deleted between pages.
#[tokio::test]
async fn scan_cursor_survives_writes() {
    for (_, s) in servers("scan_cursor_survives_writes").await {
        let mut c = Client::connect(s.addr).await;
        let mut writer = Client::connect(s.addr).await;
        let mut args: Vec<String> = vec!["MSET".into()];
        for i in 0..200 {
            args.push(format!("stable:{i:03}"));
            args.push("v".into());
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(c.call(&refs).await, Value::ok());
        let mut cursor = "0".to_owned();
        let mut seen = BTreeSet::new();
        let mut page = 0;
        loop {
            let Value::Array(reply) = c.call(&["SCAN", &cursor, "COUNT", "17"]).await else {
                panic!("a SCAN reply");
            };
            let [Value::Bulk(next), items] = reply.as_slice() else {
                panic!("cursor and items");
            };
            seen.extend(names(items));
            // Churn between pages: new keys on both sides of the cursor,
            // and deletes of churned keys.
            let a = format!("churn:{page}");
            let z = format!("zzz:{page}");
            assert_eq!(writer.call(&["SET", &a, "x"]).await, Value::ok());
            assert_eq!(writer.call(&["SET", &z, "x"]).await, Value::ok());
            if page > 0 {
                let old = format!("churn:{}", page - 1);
                assert_eq!(writer.call(&["DEL", &old]).await, int(1));
            }
            page += 1;
            cursor = String::from_utf8_lossy(next).into_owned();
            if cursor == "0" {
                break;
            }
        }
        for i in 0..200 {
            assert!(
                seen.contains(&format!("stable:{i:03}")),
                "stable:{i:03} missed"
            );
        }
    }
}

#[tokio::test]
async fn rename_moves_ttl() {
    for (_, s) in servers("rename_moves_ttl").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["SET", "a", "1", "EX", "100"]).await, Value::ok());
        assert_eq!(c.call(&["RENAME", "a", "b"]).await, Value::ok());
        assert_eq!(c.call(&["EXISTS", "a"]).await, int(0));
        assert_eq!(c.call(&["GET", "b"]).await, bulk("1"));
        assert!(matches!(c.call(&["TTL", "b"]).await, Value::Integer(n) if n > 90));
        assert_eq!(
            c.call(&["RENAME", "none", "x"]).await,
            Value::err("ERR no such key")
        );
        assert_eq!(c.call(&["RENAME", "b", "b"]).await, Value::ok());
        // A collection keeps its members under the new name.
        assert_eq!(c.call(&["ZADD", "z", "1", "m1", "2", "m2"]).await, int(2));
        assert_eq!(c.call(&["SET", "dst", "old"]).await, Value::ok());
        assert_eq!(c.call(&["RENAME", "z", "dst"]).await, Value::ok());
        assert_eq!(c.call(&["TYPE", "dst"]).await, Value::Simple("zset".into()));
        assert_eq!(c.call(&["ZSCORE", "dst", "m2"]).await, bulk("2"));
        assert_eq!(c.call(&["ZCARD", "dst"]).await, int(2));
        // RENAMENX leaves an existing destination alone.
        assert_eq!(c.call(&["SET", "p", "1"]).await, Value::ok());
        assert_eq!(c.call(&["SET", "q", "2"]).await, Value::ok());
        assert_eq!(c.call(&["RENAMENX", "p", "q"]).await, int(0));
        assert_eq!(c.call(&["GET", "q"]).await, bulk("2"));
        assert_eq!(c.call(&["RENAMENX", "p", "r"]).await, int(1));
        assert_eq!(c.call(&["GET", "r"]).await, bulk("1"));
    }
}

#[tokio::test]
async fn copy_randomkey_and_object() {
    for (_, s) in servers("copy_randomkey_and_object").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["RANDOMKEY"]).await, Value::Null);
        assert_eq!(c.call(&["SADD", "s", "a", "b"]).await, int(2));
        assert_eq!(c.call(&["COPY", "s", "t"]).await, int(1));
        assert_eq!(c.call(&["SCARD", "t"]).await, int(2));
        assert_eq!(c.call(&["SCARD", "s"]).await, int(2), "the source stays");
        assert_eq!(c.call(&["COPY", "s", "t"]).await, int(0));
        assert_eq!(c.call(&["SET", "str", "v"]).await, Value::ok());
        assert_eq!(c.call(&["COPY", "str", "t", "REPLACE"]).await, int(1));
        assert_eq!(c.call(&["TYPE", "t"]).await, Value::Simple("string".into()));
        assert_eq!(
            c.call(&["COPY", "s", "s"]).await,
            Value::err("ERR source and destination objects are the same")
        );
        assert_eq!(c.call(&["COPY", "s", "s", "DB", "1"]).await, int(1));
        assert_eq!(
            c.call(&["COPY", "s", "x", "DB", "99"]).await,
            Value::err("ERR DB index is out of range")
        );
        assert_eq!(c.call(&["COPY", "none", "x"]).await, int(0));
        let Value::Bulk(k) = c.call(&["RANDOMKEY"]).await else {
            panic!("a key");
        };
        assert!(["s", "t", "str"].contains(&&*String::from_utf8_lossy(&k)));
        assert_eq!(c.call(&["SET", "n", "12345"]).await, Value::ok());
        assert_eq!(c.call(&["OBJECT", "ENCODING", "n"]).await, bulk("int"));
        assert_eq!(c.call(&["OBJECT", "ENCODING", "str"]).await, bulk("embstr"));
        assert_eq!(
            c.call(&["OBJECT", "ENCODING", "s"]).await,
            bulk("hashtable")
        );
        assert_eq!(c.call(&["OBJECT", "ENCODING", "none"]).await, Value::Null);
        assert_eq!(
            c.call(&["OBJECT", "FREQ", "s"]).await,
            Value::err("ERR OBJECT FREQ is not supported by Loams")
        );
    }
}
