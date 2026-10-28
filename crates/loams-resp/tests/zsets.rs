//! Sorted sets over TCP (plan KV2 Task 7), on every backend.

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

fn arr(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|s| bulk(*s)).collect())
}

#[tokio::test]
async fn zadd_score_rank_and_ranges() {
    for (_, s) in servers("zadd_score_rank_and_ranges").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(
            c.call(&["ZADD", "z", "1", "a", "2", "b", "3", "c"]).await,
            int(3)
        );
        assert_eq!(
            c.call(&["ZADD", "z", "CH", "5", "a", "0.5", "d"]).await,
            int(2)
        );
        assert_eq!(c.call(&["ZADD", "z", "XX", "9", "zz"]).await, int(0));
        assert_eq!(
            c.call(&["ZADD", "z", "NX", "XX", "1", "a"]).await,
            Value::err("ERR XX and NX options at the same time are not compatible")
        );
        assert_eq!(
            c.call(&["ZADD", "z", "GT", "LT", "1", "a"]).await,
            Value::err("ERR GT, LT, and/or NX options at the same time are not compatible")
        );
        assert_eq!(
            c.call(&["ZADD", "z", "INCR", "1", "a", "2", "b"]).await,
            Value::err("ERR INCR option supports a single increment-element pair")
        );
        assert_eq!(
            c.call(&["ZADD", "z", "x", "a"]).await,
            Value::err("ERR value is not a valid float")
        );
        assert_eq!(
            c.call(&["ZADD", "z", "1"]).await,
            Value::err("ERR syntax error")
        );
        assert_eq!(c.call(&["ZCARD", "z"]).await, int(4));
        assert_eq!(c.call(&["ZSCORE", "z", "a"]).await, bulk("5"));
        assert_eq!(c.call(&["ZSCORE", "z", "none"]).await, Value::Null);
        assert_eq!(
            c.call(&["ZMSCORE", "z", "d", "none"]).await,
            Value::Array(vec![bulk("0.5"), Value::Null])
        );
        assert_eq!(c.call(&["ZINCRBY", "z", "1.5", "b"]).await, bulk("3.5"));
        // d 0.5, c 3, b 3.5, a 5
        assert_eq!(c.call(&["ZRANK", "z", "b"]).await, int(2));
        assert_eq!(c.call(&["ZREVRANK", "z", "b"]).await, int(1));
        assert_eq!(c.call(&["ZRANK", "z", "none"]).await, Value::Null);
        assert_eq!(
            c.call(&["ZRANK", "z", "b", "WITHSCORE"]).await,
            Value::Array(vec![int(2), bulk("3.5")])
        );
        assert_eq!(
            c.call(&["ZRANGE", "z", "0", "-1"]).await,
            arr(&["d", "c", "b", "a"])
        );
        assert_eq!(
            c.call(&["ZRANGE", "z", "0", "1", "REV"]).await,
            arr(&["a", "b"])
        );
        assert_eq!(
            c.call(&["ZRANGE", "z", "0", "0", "WITHSCORES"]).await,
            arr(&["d", "0.5"])
        );
        assert_eq!(
            c.call(&["ZREVRANGE", "z", "-2", "-1"]).await,
            arr(&["c", "d"])
        );
        assert_eq!(
            c.call(&["ZRANGEBYSCORE", "z", "(0.5", "3.5"]).await,
            arr(&["c", "b"])
        );
        assert_eq!(
            c.call(&["ZRANGEBYSCORE", "z", "-inf", "+inf", "LIMIT", "1", "2"])
                .await,
            arr(&["c", "b"])
        );
        assert_eq!(
            c.call(&["ZREVRANGEBYSCORE", "z", "5", "(3"]).await,
            arr(&["a", "b"])
        );
        assert_eq!(
            c.call(&["ZRANGE", "z", "(3", "+inf", "BYSCORE", "LIMIT", "0", "1"])
                .await,
            arr(&["b"])
        );
        assert_eq!(
            c.call(&["ZRANGE", "z", "+inf", "4", "BYSCORE", "REV"])
                .await,
            arr(&["a"])
        );
        assert_eq!(c.call(&["ZCOUNT", "z", "1", "4"]).await, int(2));
        assert_eq!(
            c.call(&["ZCOUNT", "z", "x", "4"]).await,
            Value::err("ERR min or max is not a float")
        );
        assert_eq!(
            c.call(&["ZRANGE", "z", "0", "1", "LIMIT", "0", "1"]).await,
            Value::err(
                "ERR syntax error, LIMIT is only supported in combination with either BYSCORE or BYLEX"
            )
        );
        // RESP3: WITHSCORES as pairs, scores as doubles.
        let Value::Map(_) = c.call(&["HELLO", "3"]).await else {
            panic!("HELLO 3");
        };
        assert_eq!(
            c.call(&["ZRANGE", "z", "0", "0", "WITHSCORES"]).await,
            Value::Array(vec![Value::Array(vec![bulk("d"), Value::Double(0.5)])])
        );
        assert_eq!(c.call(&["ZSCORE", "z", "a"]).await, Value::Double(5.0));
    }
}

#[tokio::test]
async fn zrange_bylex_bounds() {
    for (_, s) in servers("zrange_bylex_bounds").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(
            c.call(&[
                "ZADD", "l", "0", "a", "0", "b", "0", "c", "0", "d", "0", "e"
            ])
            .await,
            int(5)
        );
        assert_eq!(
            c.call(&["ZRANGEBYLEX", "l", "-", "+"]).await,
            arr(&["a", "b", "c", "d", "e"])
        );
        assert_eq!(
            c.call(&["ZRANGEBYLEX", "l", "[b", "(d"]).await,
            arr(&["b", "c"])
        );
        assert_eq!(
            c.call(&["ZREVRANGEBYLEX", "l", "+", "[c"]).await,
            arr(&["e", "d", "c"])
        );
        assert_eq!(
            c.call(&["ZRANGE", "l", "[b", "+", "BYLEX", "LIMIT", "1", "2"])
                .await,
            arr(&["c", "d"])
        );
        assert_eq!(c.call(&["ZLEXCOUNT", "l", "(a", "[c"]).await, int(2));
        assert_eq!(
            c.call(&["ZRANGEBYLEX", "l", "b", "+"]).await,
            Value::err("ERR min or max not valid string range item")
        );
        assert_eq!(c.call(&["ZREMRANGEBYLEX", "l", "[a", "[b"]).await, int(2));
        assert_eq!(c.call(&["ZCARD", "l"]).await, int(3));
    }
}

#[tokio::test]
async fn zrem_pop_and_remrange() {
    for (_, s) in servers("zrem_pop_and_remrange").await {
        let mut c = Client::connect(s.addr).await;
        let mut args: Vec<String> = vec!["ZADD".into(), "z".into()];
        for i in 0..10 {
            args.push(i.to_string());
            args.push(format!("m{i}"));
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(c.call(&refs).await, int(10));
        assert_eq!(c.call(&["ZREM", "z", "m0", "m0", "nope"]).await, int(1));
        assert_eq!(c.call(&["ZPOPMIN", "z"]).await, arr(&["m1", "1"]));
        assert_eq!(
            c.call(&["ZPOPMAX", "z", "2"]).await,
            arr(&["m9", "9", "m8", "8"])
        );
        assert_eq!(
            c.call(&["ZPOPMIN", "z", "-1"]).await,
            Value::err("ERR value is out of range, must be positive")
        );
        assert_eq!(c.call(&["ZPOPMIN", "none"]).await, Value::Array(vec![]));
        // m2..m7 remain.
        assert_eq!(c.call(&["ZREMRANGEBYRANK", "z", "0", "1"]).await, int(2));
        assert_eq!(c.call(&["ZREMRANGEBYSCORE", "z", "(4", "6"]).await, int(2));
        assert_eq!(
            c.call(&["ZRANGE", "z", "0", "-1"]).await,
            arr(&["m4", "m7"])
        );
        assert_eq!(c.call(&["ZREM", "z", "m4", "m7"]).await, int(2));
        assert_eq!(
            c.call(&["EXISTS", "z"]).await,
            int(0),
            "an emptied zset is gone"
        );
    }
}

#[tokio::test]
async fn zset_store_and_algebra() {
    for (_, s) in servers("zset_store_and_algebra").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["ZADD", "a", "1", "x", "2", "y"]).await, int(2));
        assert_eq!(c.call(&["ZADD", "b", "10", "y", "3", "z"]).await, int(2));
        assert_eq!(c.call(&["SADD", "s", "x", "w"]).await, int(2));
        assert_eq!(
            c.call(&["ZUNION", "2", "a", "b", "WITHSCORES"]).await,
            arr(&["x", "1", "z", "3", "y", "12"])
        );
        assert_eq!(
            c.call(&[
                "ZUNION",
                "2",
                "a",
                "b",
                "WEIGHTS",
                "1",
                "2",
                "AGGREGATE",
                "MAX",
                "WITHSCORES"
            ])
            .await,
            arr(&["x", "1", "z", "6", "y", "20"])
        );
        assert_eq!(c.call(&["ZINTER", "2", "a", "b"]).await, arr(&["y"]));
        assert_eq!(c.call(&["ZDIFF", "2", "a", "b"]).await, arr(&["x"]));
        // A plain set counts as scores of 1.
        assert_eq!(
            c.call(&["ZINTER", "2", "a", "s", "WITHSCORES"]).await,
            arr(&["x", "2"])
        );
        assert_eq!(c.call(&["ZINTERCARD", "2", "a", "b"]).await, int(1));
        assert_eq!(c.call(&["ZUNIONSTORE", "d", "2", "a", "b"]).await, int(3));
        assert_eq!(
            c.call(&["ZRANGE", "d", "0", "-1"]).await,
            arr(&["x", "z", "y"])
        );
        assert_eq!(
            c.call(&["ZINTERSTORE", "d", "2", "a", "none"]).await,
            int(0)
        );
        assert_eq!(c.call(&["EXISTS", "d"]).await, int(0));
        assert_eq!(c.call(&["ZRANGESTORE", "d", "b", "0", "0"]).await, int(1));
        assert_eq!(
            c.call(&["ZRANGE", "d", "0", "-1", "WITHSCORES"]).await,
            arr(&["z", "3"])
        );
        assert_eq!(
            c.call(&["ZUNION", "0", "a"]).await,
            Value::err("ERR at least 1 input key is needed for 'zunion' command")
        );
        assert_eq!(c.call(&["SET", "str", "v"]).await, Value::ok());
        assert_eq!(
            c.call(&["ZUNION", "2", "a", "str"]).await,
            Value::err("WRONGTYPE Operation against a key holding the wrong kind of value")
        );
        assert_eq!(c.call(&["ZUNIONSTORE", "str", "1", "a"]).await, int(2));
        assert_eq!(c.call(&["TYPE", "str"]).await, Value::Simple("zset".into()));
    }
}

#[tokio::test]
async fn zscan_and_zrandmember() {
    for (_, s) in servers("zscan_and_zrandmember").await {
        let mut c = Client::connect(s.addr).await;
        let mut args: Vec<String> = vec!["ZADD".into(), "big".into()];
        for i in 0..120 {
            args.push(i.to_string());
            args.push(format!("m{i:03}"));
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(c.call(&refs).await, int(120));
        let mut seen = BTreeSet::new();
        let mut cursor = "0".to_owned();
        loop {
            let Value::Array(reply) = c.call(&["ZSCAN", "big", &cursor, "COUNT", "25"]).await
            else {
                panic!("a ZSCAN reply");
            };
            let [Value::Bulk(next), Value::Array(items)] = reply.as_slice() else {
                panic!("cursor and items");
            };
            for pair in items.chunks(2) {
                assert!(seen.insert(pair[0].clone()), "a member twice");
            }
            cursor = String::from_utf8_lossy(next).into_owned();
            if cursor == "0" {
                break;
            }
        }
        assert_eq!(seen.len(), 120);
        assert_eq!(c.call(&["ZRANDMEMBER", "none"]).await, Value::Null);
        let Value::Array(three) = c.call(&["ZRANDMEMBER", "big", "3"]).await else {
            panic!("an array");
        };
        assert_eq!(three.len(), 3);
        let Value::Array(pairs) = c.call(&["ZRANDMEMBER", "big", "-4", "WITHSCORES"]).await else {
            panic!("an array");
        };
        assert_eq!(pairs.len(), 8);
    }
}
