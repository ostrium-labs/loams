use loams_cql::storage::{Cell, RowStore};
use loams_kv::{EmbeddedConfig, Store, StoreConfig};
use std::collections::BTreeMap;
async fn store(name: &str) -> RowStore {
    let kv = Store::open(StoreConfig::Embedded(EmbeddedConfig::in_memory(
        name, "cql",
    )))
    .await
    .expect("memory KV");
    RowStore::new(kv)
}
fn cells(ts: i64, value: &[u8]) -> BTreeMap<String, Cell> {
    BTreeMap::from([(
        "v".into(),
        Cell {
            timestamp: ts,
            value: value.to_vec(),
        },
    )])
}
#[tokio::test]
async fn older_cell_write_does_not_replace_newer_value() {
    let s = store("cql-lww").await;
    s.upsert(1, b"pk", b"", cells(20, b"new"))
        .await
        .expect("write");
    s.upsert(1, b"pk", b"", cells(10, b"old"))
        .await
        .expect("old write");
    assert_eq!(
        s.get(1, b"pk", b"").await.expect("read").expect("row")["v"].value,
        b"new"
    );
}
#[tokio::test]
async fn equal_timestamp_resolves_by_unsigned_value_bytes() {
    let s = store("cql-tie").await;
    s.upsert(1, b"pk", b"", cells(20, &[0xff]))
        .await
        .expect("write");
    s.upsert(1, b"pk", b"", cells(20, &[0x7f]))
        .await
        .expect("write");
    assert_eq!(
        s.get(1, b"pk", b"").await.expect("read").expect("row")["v"].value,
        [0xff]
    );
}
#[tokio::test]
async fn binary_partition_and_clustering_keys_do_not_alias() {
    let s = store("cql-keys").await;
    for (p, c, v) in [
        (b"a".as_slice(), b"\0b".as_slice(), b"one".as_slice()),
        (b"a\0".as_slice(), b"b".as_slice(), b"two".as_slice()),
    ] {
        s.upsert(1, p, c, cells(1, v)).await.expect("write");
    }
    assert_eq!(
        s.get(1, b"a", b"\0b").await.expect("read").expect("row")["v"].value,
        b"one"
    );
    assert!(s.get(2, b"a", b"\0b").await.expect("read").is_none());
}
#[tokio::test]
async fn concurrent_updates_preserve_each_column() {
    let s = store("cql-concurrent").await;
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..16 {
        let s = s.clone();
        tasks.spawn(async move {
            s.upsert(
                1,
                b"pk",
                b"",
                BTreeMap::from([(
                    format!("c{n}"),
                    Cell {
                        timestamp: n,
                        value: vec![n as u8],
                    },
                )]),
            )
            .await
            .expect("write")
        });
    }
    while let Some(v) = tasks.join_next().await {
        v.expect("join");
    }
    assert_eq!(
        s.get(1, b"pk", b"")
            .await
            .expect("read")
            .expect("row")
            .len(),
        16
    );
}
