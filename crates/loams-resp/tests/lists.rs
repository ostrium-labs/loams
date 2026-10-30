//! Lists over TCP (plan KV2 Task 8), on every backend.

use loams_kv::Store;
use loams_resp::Value;
use loams_resp::testing::{Client, TestServer, config, start};

async fn servers(name: &str, extra: &str) -> Vec<(Store, TestServer)> {
    let mut out = Vec::new();
    for store in loams_kv::testing::stores(name).await {
        let server = start(config(extra), store.clone()).await;
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
    Value::Array(items.iter().map(|s| bulk(s)).collect())
}

#[tokio::test]
async fn push_pop_range_and_index() {
    for (_, s) in servers("push_pop_range_and_index", "").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["LPUSH", "l", "a", "b", "c"]).await, int(3));
        assert_eq!(c.call(&["RPUSH", "l", "d", "e"]).await, int(5));
        assert_eq!(
            c.call(&["LRANGE", "l", "0", "-1"]).await,
            arr(&["c", "b", "a", "d", "e"])
        );
        assert_eq!(
            c.call(&["LRANGE", "l", "-2", "100"]).await,
            arr(&["d", "e"])
        );
        assert_eq!(c.call(&["LRANGE", "l", "3", "1"]).await, arr(&[]));
        assert_eq!(c.call(&["LLEN", "l"]).await, int(5));
        assert_eq!(c.call(&["LINDEX", "l", "1"]).await, bulk("b"));
        assert_eq!(c.call(&["LINDEX", "l", "-1"]).await, bulk("e"));
        assert_eq!(c.call(&["LINDEX", "l", "10"]).await, Value::Null);
        assert_eq!(c.call(&["LPUSHX", "none", "x"]).await, int(0));
        assert_eq!(c.call(&["EXISTS", "none"]).await, int(0));
        assert_eq!(c.call(&["RPUSHX", "l", "f"]).await, int(6));
        assert_eq!(c.call(&["LPOP", "l"]).await, bulk("c"));
        assert_eq!(c.call(&["RPOP", "l", "2"]).await, arr(&["f", "e"]));
        assert_eq!(c.call(&["LPOP", "l", "0"]).await, arr(&[]));
        assert_eq!(c.call(&["LPOP", "none"]).await, Value::Null);
        assert_eq!(c.call(&["LPOP", "none", "2"]).await, Value::NullArray);
        assert_eq!(
            c.call(&["LPOP", "l", "-1"]).await,
            Value::err("ERR value is out of range, must be positive")
        );
        assert_eq!(c.call(&["RPOP", "l", "10"]).await, arr(&["d", "a", "b"]));
        assert_eq!(
            c.call(&["EXISTS", "l"]).await,
            int(0),
            "an emptied list is gone"
        );
        assert_eq!(c.call(&["SET", "s", "v"]).await, Value::ok());
        assert_eq!(
            c.call(&["LPUSH", "s", "x"]).await,
            Value::err("WRONGTYPE Operation against a key holding the wrong kind of value")
        );
    }
}

#[tokio::test]
async fn set_trim_insert_rem_pos() {
    for (_, s) in servers("set_trim_insert_rem_pos", "").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(
            c.call(&["RPUSH", "l", "a", "b", "c", "b", "e"]).await,
            int(5)
        );
        assert_eq!(c.call(&["LSET", "l", "0", "A"]).await, Value::ok());
        assert_eq!(
            c.call(&["LSET", "l", "9", "x"]).await,
            Value::err("ERR index out of range")
        );
        assert_eq!(
            c.call(&["LSET", "none", "0", "x"]).await,
            Value::err("ERR no such key")
        );
        assert_eq!(c.call(&["LINSERT", "l", "AFTER", "c", "C"]).await, int(6));
        assert_eq!(
            c.call(&["LINSERT", "l", "BEFORE", "zz", "x"]).await,
            int(-1)
        );
        assert_eq!(
            c.call(&["LINSERT", "none", "BEFORE", "a", "x"]).await,
            int(0)
        );
        assert_eq!(
            c.call(&["LINSERT", "l", "AROUND", "a", "x"]).await,
            Value::err("ERR syntax error")
        );
        assert_eq!(
            c.call(&["LRANGE", "l", "0", "-1"]).await,
            arr(&["A", "b", "c", "C", "b", "e"])
        );
        assert_eq!(c.call(&["LPOS", "l", "b"]).await, int(1));
        assert_eq!(c.call(&["LPOS", "l", "b", "RANK", "-1"]).await, int(4));
        assert_eq!(
            c.call(&["LPOS", "l", "b", "COUNT", "0"]).await,
            Value::Array(vec![int(1), int(4)])
        );
        assert_eq!(c.call(&["LPOS", "l", "zz"]).await, Value::Null);
        assert_eq!(
            c.call(&["LPOS", "l", "b", "RANK", "0"]).await,
            Value::err(
                "ERR RANK can't be zero: use 1 to start from the first match, 2 from the second ... or use negative to start from the last match"
            )
        );
        assert_eq!(c.call(&["LREM", "l", "-1", "b"]).await, int(1));
        assert_eq!(
            c.call(&["LRANGE", "l", "0", "-1"]).await,
            arr(&["A", "b", "c", "C", "e"])
        );
        assert_eq!(c.call(&["LTRIM", "l", "1", "-2"]).await, Value::ok());
        assert_eq!(
            c.call(&["LRANGE", "l", "0", "-1"]).await,
            arr(&["b", "c", "C"])
        );
        assert_eq!(c.call(&["LTRIM", "l", "5", "9"]).await, Value::ok());
        assert_eq!(c.call(&["EXISTS", "l"]).await, int(0));
    }
}

#[tokio::test]
async fn lmove_and_rpoplpush() {
    for (_, s) in servers("lmove_and_rpoplpush", "").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["RPUSH", "a", "1", "2", "3"]).await, int(3));
        assert_eq!(
            c.call(&["LMOVE", "a", "b", "LEFT", "RIGHT"]).await,
            bulk("1")
        );
        assert_eq!(c.call(&["RPOPLPUSH", "a", "b"]).await, bulk("3"));
        assert_eq!(c.call(&["LRANGE", "b", "0", "-1"]).await, arr(&["3", "1"]));
        // Rotation on one list.
        assert_eq!(c.call(&["RPUSH", "r", "x", "y", "z"]).await, int(3));
        assert_eq!(
            c.call(&["LMOVE", "r", "r", "LEFT", "RIGHT"]).await,
            bulk("x")
        );
        assert_eq!(
            c.call(&["LRANGE", "r", "0", "-1"]).await,
            arr(&["y", "z", "x"])
        );
        assert_eq!(
            c.call(&["LMOVE", "none", "b", "LEFT", "LEFT"]).await,
            Value::Null
        );
        assert_eq!(
            c.call(&["LMOVE", "a", "b", "UP", "LEFT"]).await,
            Value::err("ERR syntax error")
        );
        assert_eq!(c.call(&["SET", "s", "v"]).await, Value::ok());
        assert_eq!(
            c.call(&["LMOVE", "a", "s", "LEFT", "LEFT"]).await,
            Value::err("WRONGTYPE Operation against a key holding the wrong kind of value")
        );
        assert_eq!(c.call(&["LLEN", "a"]).await, int(1), "nothing moved");
    }
}

#[tokio::test]
async fn linsert_shift_limit() {
    for (_, s) in servers("linsert_shift_limit", "list_max_shift = 4").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(
            c.call(&["RPUSH", "l", "a", "b", "c", "d", "e"]).await,
            int(5)
        );
        assert_eq!(
            c.call(&["LINSERT", "l", "BEFORE", "c", "x"]).await,
            Value::err(
                "ERR LINSERT on a list longer than list-max-shift (4) is not supported by Loams"
            )
        );
        assert_eq!(
            c.call(&["LREM", "l", "0", "c"]).await,
            Value::err(
                "ERR LREM on a list longer than list-max-shift (4) is not supported by Loams"
            )
        );
        // Pushes and pops are not limited.
        assert_eq!(c.call(&["RPUSH", "l", "f"]).await, int(6));
    }
}
