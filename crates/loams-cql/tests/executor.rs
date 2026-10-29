use loams_cql::{
    executor::KvExecutor,
    query::{QueryExecutor, QueryResult, SessionContext},
};
use loams_kv::{EmbeddedConfig, Store, StoreConfig};
async fn executor(name: &str) -> (KvExecutor, SessionContext) {
    let kv = Store::open(StoreConfig::Embedded(EmbeddedConfig::in_memory(
        name, "cql-exec",
    )))
    .await
    .expect("store");
    (
        KvExecutor::new("tenant", kv),
        SessionContext {
            namespace: "tenant".into(),
            principal: "worker".into(),
            keyspace: Some("ks".into()),
        },
    )
}
async fn schema(e: &KvExecutor, s: &SessionContext) {
    e.execute(
        s,
        "CREATE KEYSPACE ks WITH replication = {'class':'SimpleStrategy','replication_factor':1}",
        &[],
    )
    .await
    .expect("keyspace");
    e.execute(
        s,
        "CREATE TABLE ks.items (id bigint PRIMARY KEY, value text)",
        &[],
    )
    .await
    .expect("table");
}
#[tokio::test]
async fn creates_inserts_and_selects_bound_values() {
    let (e, s) = executor("exec-bind").await;
    schema(&e, &s).await;
    e.execute(
        &s,
        "INSERT INTO ks.items (id,value) VALUES (?,?)",
        &[Some(42i64.to_be_bytes().to_vec()), Some(b"answer".to_vec())],
    )
    .await
    .expect("insert");
    let r = e
        .execute(
            &s,
            "SELECT value FROM ks.items WHERE id = ?",
            &[Some(42i64.to_be_bytes().to_vec())],
        )
        .await
        .expect("select");
    let QueryResult::Rows { rows, .. } = r else {
        panic!("rows")
    };
    assert_eq!(rows, vec![vec![Some(b"answer".to_vec())]]);
}
#[tokio::test]
async fn rejects_cross_namespace_and_wrong_primary_key() {
    let (e, mut s) = executor("exec-namespace").await;
    schema(&e, &s).await;
    assert!(
        e.execute(&s, "SELECT * FROM ks.items WHERE value = 'x'", &[])
            .await
            .is_err()
    );
    s.namespace = "other".into();
    assert!(
        e.execute(&s, "SELECT * FROM ks.items WHERE id = 1", &[])
            .await
            .is_err()
    );
}
#[tokio::test]
async fn recreate_table_does_not_read_dropped_rows() {
    let (e, s) = executor("exec-drop").await;
    schema(&e, &s).await;
    e.execute(&s, "INSERT INTO ks.items (id,value) VALUES (1,'old')", &[])
        .await
        .expect("insert");
    e.execute(&s, "DROP TABLE ks.items", &[])
        .await
        .expect("drop");
    e.execute(
        &s,
        "CREATE TABLE ks.items (id bigint PRIMARY KEY,value text)",
        &[],
    )
    .await
    .expect("create");
    let QueryResult::Rows { rows, .. } = e
        .execute(&s, "SELECT * FROM ks.items WHERE id = 1", &[])
        .await
        .expect("query")
    else {
        panic!("rows")
    };
    assert!(rows.is_empty());
}
#[tokio::test]
async fn invalid_bound_integer_does_not_write() {
    let (e, s) = executor("exec-invalid").await;
    schema(&e, &s).await;
    assert!(
        e.execute(
            &s,
            "INSERT INTO ks.items (id,value) VALUES (?,?)",
            &[Some(vec![1]), Some(b"bad".to_vec())]
        )
        .await
        .is_err()
    );
}
