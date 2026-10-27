//! Counters over TCP (plan KV2 Task 4), on every backend.

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

#[tokio::test]
async fn incr_overflow_and_parse_errors() {
    for (_, s) in servers("incr_overflow_and_parse_errors").await {
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["INCR", "n"]).await, int(1));
        assert_eq!(c.call(&["INCRBY", "n", "10"]).await, int(11));
        assert_eq!(c.call(&["DECR", "n"]).await, int(10));
        assert_eq!(c.call(&["DECRBY", "n", "15"]).await, int(-5));
        assert_eq!(c.call(&["GET", "n"]).await, Value::bulk("-5".to_owned()));
        assert_eq!(
            c.call(&["INCRBY", "n", "x"]).await,
            Value::err("ERR value is not an integer or out of range")
        );
        assert_eq!(
            c.call(&["DECRBY", "n", "-9223372036854775808"]).await,
            Value::err("ERR decrement would overflow")
        );
        assert_eq!(
            c.call(&["SET", "max", "9223372036854775807"]).await,
            Value::ok()
        );
        assert_eq!(
            c.call(&["INCR", "max"]).await,
            Value::err("ERR increment or decrement would overflow")
        );
        assert_eq!(c.call(&["SET", "s", " 11"]).await, Value::ok());
        assert_eq!(
            c.call(&["INCR", "s"]).await,
            Value::err("ERR value is not an integer or out of range")
        );
        // The TTL survives an increment.
        assert_eq!(c.call(&["SET", "t", "1", "EX", "100"]).await, Value::ok());
        assert_eq!(c.call(&["INCR", "t"]).await, int(2));
        assert!(matches!(c.call(&["TTL", "t"]).await, Value::Integer(n) if n > 90));
    }
}

#[tokio::test]
async fn incrbyfloat_formatting() {
    for (_, s) in servers("incrbyfloat_formatting").await {
        let mut c = Client::connect(s.addr).await;
        let bulk = |t: &str| Value::bulk(t.to_owned());
        assert_eq!(c.call(&["INCRBYFLOAT", "f", "10.5"]).await, bulk("10.5"));
        assert_eq!(c.call(&["INCRBYFLOAT", "f", "0.1"]).await, bulk("10.6"));
        assert_eq!(c.call(&["INCRBYFLOAT", "f", "-10.6"]).await, bulk("0"));
        assert_eq!(c.call(&["SET", "e", "5.0e3"]).await, Value::ok());
        assert_eq!(c.call(&["INCRBYFLOAT", "e", "2.0e2"]).await, bulk("5200"));
        assert_eq!(
            c.call(&["INCRBYFLOAT", "e", "x"]).await,
            Value::err("ERR value is not a valid float")
        );
        assert_eq!(
            c.call(&["INCRBYFLOAT", "e", "+inf"]).await,
            Value::err("ERR increment would produce NaN or Infinity")
        );
        assert_eq!(c.call(&["SET", "w", "1 "]).await, Value::ok());
        assert_eq!(
            c.call(&["INCRBYFLOAT", "w", "1"]).await,
            Value::err("ERR value is not a valid float")
        );
    }
}

/// Sixteen clients each `INCR` one key a hundred times; no update is lost
/// (the runner retries conflicting transactions).
#[tokio::test]
async fn concurrent_incr_loses_no_update() {
    for (_, s) in servers("concurrent_incr_loses_no_update").await {
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let addr = s.addr;
            tasks.push(tokio::spawn(async move {
                let mut c = Client::connect(addr).await;
                for _ in 0..100 {
                    assert!(matches!(c.call(&["INCR", "hot"]).await, Value::Integer(_)));
                }
            }));
        }
        for t in tasks {
            t.await.expect("a client task");
        }
        let mut c = Client::connect(s.addr).await;
        assert_eq!(
            c.call(&["GET", "hot"]).await,
            Value::bulk("1600".to_owned())
        );
    }
}
