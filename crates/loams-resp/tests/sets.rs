//! Sets over TCP (plan KV2 Task 6), on every backend.

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

fn int(n: i64) -> Value {
    Value::Integer(n)
}

/// The members of a reply (an array in RESP2), as strings.
fn names(v: &Value) -> BTreeSet<String> {
    let (Value::Array(items) | Value::Set(items)) = v else {
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

#[tokio::test]
async fn set_commands() {
    for (_, s) in servers("set_commands").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["SADD", "s", "a", "b", "c", "a"]).await, int(3));
        assert_eq!(c.call(&["SADD", "s", "c", "d"]).await, int(1));
        assert_eq!(c.call(&["SCARD", "s"]).await, int(4));
        assert_eq!(c.call(&["SCARD", "none"]).await, int(0));
        assert_eq!(
            names(&c.call(&["SMEMBERS", "s"]).await),
            set_of(&["a", "b", "c", "d"])
        );
        assert_eq!(c.call(&["SISMEMBER", "s", "a"]).await, int(1));
        assert_eq!(c.call(&["SISMEMBER", "s", "z"]).await, int(0));
        assert_eq!(
            c.call(&["SMISMEMBER", "s", "a", "z", "d"]).await,
            Value::Array(vec![int(1), int(0), int(1)])
        );
        assert_eq!(c.call(&["SREM", "s", "a", "z"]).await, int(1));
        assert_eq!(c.call(&["SMOVE", "s", "t", "b"]).await, int(1));
        assert_eq!(c.call(&["SMOVE", "s", "t", "zz"]).await, int(0));
        assert_eq!(names(&c.call(&["SMEMBERS", "t"]).await), set_of(&["b"]));
        assert_eq!(c.call(&["SREM", "t", "b"]).await, int(1));
        assert_eq!(
            c.call(&["EXISTS", "t"]).await,
            int(0),
            "an emptied set is gone"
        );
        // RESP3 replies with a set type.
        let Value::Map(_) = c.call(&["HELLO", "3"]).await else {
            panic!("HELLO 3");
        };
        assert!(matches!(c.call(&["SMEMBERS", "s"]).await, Value::Set(_)));
    }
}

#[tokio::test]
async fn set_algebra_commands() {
    for (_, s) in servers("set_algebra_commands").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["SADD", "a", "1", "2", "3", "4"]).await, int(4));
        assert_eq!(c.call(&["SADD", "b", "3", "4", "5"]).await, int(3));
        assert_eq!(c.call(&["SADD", "c", "4", "6"]).await, int(2));
        assert_eq!(
            names(&c.call(&["SINTER", "a", "b"]).await),
            set_of(&["3", "4"])
        );
        assert_eq!(
            names(&c.call(&["SINTER", "a", "b", "c"]).await),
            set_of(&["4"])
        );
        assert_eq!(names(&c.call(&["SINTER", "a", "none"]).await), set_of(&[]));
        assert_eq!(
            names(&c.call(&["SUNION", "a", "c", "none"]).await),
            set_of(&["1", "2", "3", "4", "6"])
        );
        assert_eq!(
            names(&c.call(&["SDIFF", "a", "b", "c"]).await),
            set_of(&["1", "2"])
        );
        assert_eq!(c.call(&["SINTERCARD", "2", "a", "b"]).await, int(2));
        assert_eq!(
            c.call(&["SINTERCARD", "2", "a", "b", "LIMIT", "1"]).await,
            int(1)
        );
        assert_eq!(
            c.call(&["SINTERCARD", "0", "a"]).await,
            Value::err("ERR numkeys should be greater than 0")
        );
        assert_eq!(
            c.call(&["SINTERCARD", "3", "a", "b"]).await,
            Value::err("ERR Number of keys can't be greater than number of args")
        );
        assert_eq!(
            c.call(&["SINTERCARD", "2", "a", "b", "LIMIT", "-1"]).await,
            Value::err("ERR LIMIT can't be negative")
        );
        assert_eq!(c.call(&["SINTERSTORE", "d", "a", "b"]).await, int(2));
        assert_eq!(
            names(&c.call(&["SMEMBERS", "d"]).await),
            set_of(&["3", "4"])
        );
        assert_eq!(c.call(&["SUNIONSTORE", "d", "b", "c"]).await, int(4));
        assert_eq!(c.call(&["SDIFFSTORE", "d", "a", "a"]).await, int(0));
        assert_eq!(
            c.call(&["EXISTS", "d"]).await,
            int(0),
            "an empty result deletes the key"
        );
        // A store replaces a key of another type.
        assert_eq!(c.call(&["SET", "str", "v"]).await, Value::ok());
        assert_eq!(c.call(&["SUNIONSTORE", "str", "c"]).await, int(2));
        assert_eq!(c.call(&["TYPE", "str"]).await, Value::Simple("set".into()));
        // A source of another type is an error.
        assert_eq!(c.call(&["SET", "x", "v"]).await, Value::ok());
        assert_eq!(
            c.call(&["SINTER", "a", "x"]).await,
            Value::err("WRONGTYPE Operation against a key holding the wrong kind of value")
        );
    }
}

#[tokio::test]
async fn spop_count_and_srandmember_negative() {
    for (_, s) in servers("spop_count_and_srandmember_negative").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["SPOP", "none"]).await, Value::Null);
        assert_eq!(c.call(&["SRANDMEMBER", "none"]).await, Value::Null);
        assert_eq!(
            c.call(&["SRANDMEMBER", "none", "3"]).await,
            Value::Array(vec![])
        );
        assert_eq!(
            c.call(&["SADD", "s", "a", "b", "c", "d", "e"]).await,
            int(5)
        );
        let all = set_of(&["a", "b", "c", "d", "e"]);
        let Value::Bulk(one) = c.call(&["SRANDMEMBER", "s"]).await else {
            panic!("a member");
        };
        assert!(all.contains(&*String::from_utf8_lossy(&one)));
        assert_eq!(names(&c.call(&["SRANDMEMBER", "s", "10"]).await), all);
        let Value::Array(reps) = c.call(&["SRANDMEMBER", "s", "-20"]).await else {
            panic!("an array");
        };
        assert_eq!(reps.len(), 20);
        assert_eq!(
            c.call(&["SPOP", "s", "-1"]).await,
            Value::err("ERR value is out of range, must be positive")
        );
        let two = names(&c.call(&["SPOP", "s", "2"]).await);
        assert_eq!(two.len(), 2);
        assert!(two.is_subset(&all));
        assert_eq!(c.call(&["SCARD", "s"]).await, int(3));
        let Value::Bulk(_) = c.call(&["SPOP", "s"]).await else {
            panic!("a member");
        };
        let rest = names(&c.call(&["SPOP", "s", "10"]).await);
        assert_eq!(rest.len(), 2);
        assert_eq!(c.call(&["EXISTS", "s"]).await, int(0));
    }
}

#[tokio::test]
async fn sscan_returns_every_member_once() {
    for (_, s) in servers("sscan_returns_every_member_once").await {
        let mut c = Client::connect(s.addr).await;
        let mut args: Vec<String> = vec!["SADD".into(), "big".into()];
        args.extend((0..300).map(|i| format!("m{i:03}")));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(c.call(&refs).await, int(300));
        let mut seen = BTreeSet::new();
        let mut cursor = "0".to_owned();
        loop {
            let Value::Array(reply) = c.call(&["SSCAN", "big", &cursor, "COUNT", "50"]).await
            else {
                panic!("an SSCAN reply");
            };
            let [Value::Bulk(next), items] = reply.as_slice() else {
                panic!("cursor and items");
            };
            for m in names(items) {
                assert!(seen.insert(m), "a member twice");
            }
            cursor = String::from_utf8_lossy(next).into_owned();
            if cursor == "0" {
                break;
            }
        }
        assert_eq!(seen.len(), 300);
        assert_eq!(
            c.call(&["SSCAN", "big", "0", "NOVALUES"]).await,
            Value::err("ERR syntax error")
        );
    }
}
