//! The expiry and GC workers through a running gateway (plan KV2 Task 10).

use std::time::{Duration, Instant};

use loams_resp::Value;
use loams_resp::layout::{self, HASH_FIELD, Meta};
use loams_resp::testing::{Client, config, start};

/// Waits up to five seconds for `check` to hold.
async fn eventually<F, Fut>(mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

/// Expired keys are deleted, and an expired hash's fields collected, with
/// no client ever reading them again.
#[tokio::test]
async fn expired_keys_are_reclaimed_without_reads() {
    for store in loams_kv::testing::stores("workers_reclaim").await {
        let s = start(
            config("expire_interval_ms = 20\ngc_interval_ms = 40"),
            store.clone(),
        )
        .await;
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["SET", "s", "v", "PX", "60"]).await, Value::ok());
        assert_eq!(
            c.call(&["HSET", "h", "f", "v", "g", "w"]).await,
            Value::Integer(2)
        );
        let meta = store
            .get(&layout::meta_key(0, b"h"))
            .await
            .expect("a get")
            .expect("the hash's meta");
        let ver = Meta::decode(&meta).expect("a meta record").ver;
        let field = layout::item_key(0, b"h", ver, HASH_FIELD, b"f");
        assert!(store.get(&field).await.expect("a get").is_some());
        assert_eq!(c.call(&["PEXPIRE", "h", "60"]).await, Value::Integer(1));
        assert_eq!(c.call(&["SET", "keep", "v"]).await, Value::ok());
        let gone = |k: Vec<u8>| {
            let store = store.clone();
            async move { store.get(&k).await.expect("a get").is_none() }
        };
        assert!(
            eventually(|| gone(layout::meta_key(0, b"s"))).await,
            "the string is reclaimed"
        );
        assert!(
            eventually(|| gone(layout::meta_key(0, b"h"))).await,
            "the hash is reclaimed"
        );
        assert!(
            eventually(|| gone(field.clone())).await,
            "its fields are collected"
        );
        assert!(
            eventually(|| gone(layout::gc_key(0, b"h", ver))).await,
            "its queue entry too"
        );
        assert_eq!(c.call(&["GET", "keep"]).await, Value::bulk("v".to_owned()));
        let text = loams_resp::metrics::metrics().render();
        assert!(text.contains("loams_resp_expired_total"), "{text}");
    }
}

/// A key overwritten without a TTL outlives its old expiry.
#[tokio::test]
async fn overwritten_key_is_not_expired_by_stale_index() {
    for store in loams_kv::testing::stores("workers_stale_index").await {
        let s = start(config("expire_interval_ms = 20"), store.clone()).await;
        let mut c = Client::connect(s.addr).await;
        assert_eq!(c.call(&["SET", "k", "old", "PX", "50"]).await, Value::ok());
        assert_eq!(c.call(&["SET", "k", "new"]).await, Value::ok());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(c.call(&["GET", "k"]).await, Value::bulk("new".to_owned()));
        assert_eq!(c.call(&["TTL", "k"]).await, Value::Integer(-1));
    }
}
