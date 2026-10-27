//! Hashes over TCP (plan KV2 Task 5), on every backend.

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

#[tokio::test]
async fn hash_commands() {
    for (_, s) in servers("hash_commands").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["HSET", "h", "a", "1", "b", "2"]).await, int(2));
        assert_eq!(c.call(&["HSET", "h", "a", "10", "c", "3"]).await, int(1));
        assert_eq!(
            c.call(&["HSET", "h", "a"]).await,
            Value::err("ERR wrong number of arguments for 'hset' command")
        );
        assert_eq!(c.call(&["HMSET", "h", "d", "4"]).await, Value::ok());
        assert_eq!(c.call(&["HSETNX", "h", "a", "x"]).await, int(0));
        assert_eq!(c.call(&["HSETNX", "h", "e", "5"]).await, int(1));
        assert_eq!(c.call(&["HGET", "h", "a"]).await, bulk("10"));
        assert_eq!(c.call(&["HGET", "h", "zz"]).await, Value::Null);
        assert_eq!(c.call(&["HGET", "nokey", "a"]).await, Value::Null);
        assert_eq!(
            c.call(&["HMGET", "h", "b", "zz", "c"]).await,
            Value::Array(vec![bulk("2"), Value::Null, bulk("3")])
        );
        assert_eq!(c.call(&["HLEN", "h"]).await, int(5));
        assert_eq!(c.call(&["HEXISTS", "h", "b"]).await, int(1));
        assert_eq!(c.call(&["HEXISTS", "h", "zz"]).await, int(0));
        assert_eq!(c.call(&["HSTRLEN", "h", "a"]).await, int(2));
        assert_eq!(
            c.call(&["HKEYS", "h"]).await,
            Value::Array(["a", "b", "c", "d", "e"].map(bulk).to_vec())
        );
        assert_eq!(
            c.call(&["HVALS", "h"]).await,
            Value::Array(["10", "2", "3", "4", "5"].map(bulk).to_vec())
        );
        // HGETALL: a flat array in RESP2, a map in RESP3.
        let Value::Array(flat) = c.call(&["HGETALL", "h"]).await else {
            panic!("a flat array");
        };
        assert_eq!(flat.len(), 10);
        let Value::Map(_) = c.call(&["HELLO", "3"]).await else {
            panic!("HELLO 3");
        };
        let Value::Map(map) = c.call(&["HGETALL", "h"]).await else {
            panic!("a map");
        };
        assert_eq!(map.len(), 5);
        assert_eq!(c.call(&["HDEL", "h", "a", "a", "zz"]).await, int(1));
        assert_eq!(c.call(&["HDEL", "h", "b", "c", "d", "e"]).await, int(4));
        assert_eq!(
            c.call(&["EXISTS", "h"]).await,
            int(0),
            "an emptied hash is gone"
        );
        assert_eq!(c.call(&["TYPE", "h"]).await, Value::Simple("none".into()));
    }
}

#[tokio::test]
async fn hash_counters() {
    for (_, s) in servers("hash_counters").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["HINCRBY", "h", "n", "5"]).await, int(5));
        assert_eq!(c.call(&["HINCRBY", "h", "n", "-7"]).await, int(-2));
        assert_eq!(c.call(&["HSET", "h", "s", "abc"]).await, int(1));
        assert_eq!(
            c.call(&["HINCRBY", "h", "s", "1"]).await,
            Value::err("ERR hash value is not an integer")
        );
        assert_eq!(
            c.call(&["HINCRBY", "h", "n", "x"]).await,
            Value::err("ERR value is not an integer or out of range")
        );
        assert_eq!(
            c.call(&["HINCRBY", "h", "n", "9223372036854775807"]).await,
            Value::err("ERR increment or decrement would overflow")
        );
        assert_eq!(
            c.call(&["HINCRBYFLOAT", "h", "f", "10.5"]).await,
            bulk("10.5")
        );
        assert_eq!(
            c.call(&["HINCRBYFLOAT", "h", "f", "0.1"]).await,
            bulk("10.6")
        );
        assert_eq!(
            c.call(&["HINCRBYFLOAT", "h", "s", "1"]).await,
            Value::err("ERR hash value is not a float")
        );
        assert_eq!(
            c.call(&["HINCRBYFLOAT", "h", "f", "nan"]).await,
            Value::err("ERR value is not a valid float")
        );
        assert_eq!(c.call(&["HLEN", "h"]).await, int(3));
    }
}

#[tokio::test]
async fn hset_on_string_is_wrongtype() {
    for (_, s) in servers("hset_on_string_is_wrongtype").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["SET", "s", "v"]).await, Value::ok());
        let wrong = Value::err("WRONGTYPE Operation against a key holding the wrong kind of value");
        for cmd in [
            &["HSET", "s", "f", "v"][..],
            &["HGET", "s", "f"],
            &["HGETALL", "s"],
            &["HLEN", "s"],
            &["HDEL", "s", "f"],
            &["HINCRBY", "s", "f", "1"],
            &["HSCAN", "s", "0"],
            &["HRANDFIELD", "s"],
        ] {
            assert_eq!(c.call(cmd).await, wrong, "{cmd:?}");
        }
        // And the other way: string commands on a hash.
        assert_eq!(c.call(&["HSET", "h", "f", "v"]).await, int(1));
        assert_eq!(c.call(&["GET", "h"]).await, wrong);
        // SET replaces the hash; its fields do not come back.
        assert_eq!(c.call(&["SET", "h", "v"]).await, Value::ok());
        assert_eq!(c.call(&["DEL", "h"]).await, int(1));
        assert_eq!(c.call(&["HSET", "h", "g", "1"]).await, int(1));
        assert_eq!(c.call(&["HGET", "h", "f"]).await, Value::Null);
        assert_eq!(c.call(&["HLEN", "h"]).await, int(1));
    }
}

#[tokio::test]
async fn hscan_returns_every_field_once() {
    for (_, s) in servers("hscan_returns_every_field_once").await {
        let mut c = Client::connect(s.addr).await;
        let mut args: Vec<String> = vec!["HSET".into(), "big".into()];
        for i in 0..250 {
            args.push(format!("f{i:03}"));
            args.push(format!("v{i}"));
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(c.call(&refs).await, int(250));
        let mut seen = BTreeSet::new();
        let mut cursor = "0".to_owned();
        let mut rounds = 0;
        loop {
            rounds += 1;
            let Value::Array(reply) = c.call(&["HSCAN", "big", &cursor, "COUNT", "40"]).await
            else {
                panic!("an HSCAN reply");
            };
            let [Value::Bulk(next), Value::Array(items)] = reply.as_slice() else {
                panic!("cursor and items");
            };
            for pair in items.chunks(2) {
                let Value::Bulk(f) = &pair[0] else {
                    panic!("a field");
                };
                assert!(seen.insert(f.clone()), "{f:?} twice");
            }
            cursor = String::from_utf8_lossy(next).into_owned();
            if cursor == "0" {
                break;
            }
        }
        assert_eq!(seen.len(), 250);
        assert!(rounds >= 7, "{rounds} rounds for 250 fields at COUNT 40");
        // MATCH and NOVALUES.
        let Value::Array(reply) = c
            .call(&[
                "HSCAN", "big", "0", "MATCH", "f00*", "COUNT", "1000", "NOVALUES",
            ])
            .await
        else {
            panic!("an HSCAN reply");
        };
        assert_eq!(reply[0], bulk("0"));
        let Value::Array(items) = &reply[1] else {
            panic!("items");
        };
        assert_eq!(items.len(), 10, "f000..f009, fields only");
        assert_eq!(
            c.call(&["HSCAN", "big", "x"]).await,
            Value::err("ERR invalid cursor")
        );
    }
}

#[tokio::test]
async fn hrandfield_counts() {
    for (_, s) in servers("hrandfield_counts").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["HRANDFIELD", "none"]).await, Value::Null);
        assert_eq!(
            c.call(&["HRANDFIELD", "none", "5"]).await,
            Value::Array(vec![])
        );
        assert_eq!(
            c.call(&["HSET", "h", "a", "1", "b", "2", "c", "3"]).await,
            int(3)
        );
        assert!(matches!(c.call(&["HRANDFIELD", "h"]).await, Value::Bulk(_)));
        let Value::Array(all) = c.call(&["HRANDFIELD", "h", "10"]).await else {
            panic!("an array");
        };
        assert_eq!(all.len(), 3);
        let Value::Array(two) = c.call(&["HRANDFIELD", "h", "2"]).await else {
            panic!("an array");
        };
        assert_eq!(two.len(), 2);
        assert_ne!(two[0], two[1]);
        let Value::Array(reps) = c.call(&["HRANDFIELD", "h", "-7"]).await else {
            panic!("an array");
        };
        assert_eq!(reps.len(), 7);
        let Value::Array(flat) = c.call(&["HRANDFIELD", "h", "2", "WITHVALUES"]).await else {
            panic!("an array");
        };
        assert_eq!(flat.len(), 4, "field, value, field, value in RESP2");
        assert_eq!(
            c.call(&["HRANDFIELD", "h", "2", "VALUES"]).await,
            Value::err("ERR syntax error")
        );
        assert_eq!(
            c.call(&["HRANDFIELD", "h", "-9223372036854775808", "WITHVALUES"])
                .await,
            Value::err("ERR value is out of range")
        );
    }
}
