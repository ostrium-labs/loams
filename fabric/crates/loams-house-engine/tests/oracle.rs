//! Execution, snapshot and HTTP contract checks for the development oracle.
#![allow(clippy::unwrap_used)]
use datafusion::arrow::{
    array::{RecordBatch, StringArray, UInt8Array},
    datatypes::Schema,
};
use loams_house_engine::{
    catalog::MemoryCatalog,
    execute::{compact_json, execute},
    fixtures::{Column, Snapshot, Table},
};
use loams_house_types::{ChType, arrow::to_arrow};
use parquet::arrow::ArrowWriter;
use std::{fs::File, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn snapshot(root: &std::path::Path) {
    let schema = Arc::new(Schema::new(vec![
        to_arrow("k", &ChType::UInt8).unwrap(),
        to_arrow("s", &ChType::String).unwrap(),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt8Array::from(vec![1, 2])),
            Arc::new(StringArray::from(vec!["hello", "world"])),
        ],
    )
    .unwrap();
    let mut writer = ArrowWriter::try_new(
        File::create(root.join("rows.parquet")).unwrap(),
        schema,
        None,
    )
    .unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    let snapshot = Snapshot {
        tables: vec![Table {
            database: "default".into(),
            name: "t".into(),
            engine: "ReplacingMergeTree".into(),
            sorting_key: "k".into(),
            partition_key: "".into(),
            columns: vec![
                Column {
                    name: "k".into(),
                    r#type: "UInt8".into(),
                    default_kind: "".into(),
                    default_expression: "".into(),
                },
                Column {
                    name: "s".into(),
                    r#type: "String".into(),
                    default_kind: "DEFAULT".into(),
                    default_expression: "'x'".into(),
                },
            ],
            parquet: "rows.parquet".into(),
        }],
    };
    std::fs::write(
        root.join("snapshot.json"),
        serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
}
#[tokio::test]
async fn seeded_tables_read_back_as_the_oracle_wrote_them() {
    let dir = tempfile::tempdir().unwrap();
    snapshot(dir.path());
    let (metadata, catalog) = Snapshot::load(dir.path()).unwrap();
    assert_eq!(metadata.tables[0].engine, "ReplacingMergeTree");
    assert_eq!(metadata.tables[0].columns[1].default_expression, "'x'");
    let got = execute("SELECT * FROM t ORDER BY k", &[], "default", &catalog)
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(got).unwrap(),
        "[\"k\",\"s\"]\n[\"UInt8\",\"String\"]\n[1,\"hello\"]\n[2,\"world\"]\n"
    );
    let got = execute("SELECT * FROM t WHERE k = 99", &[], "default", &catalog)
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(got).unwrap(),
        "[\"k\",\"s\"]\n[\"UInt8\",\"String\"]\n"
    );
}
#[test]
fn zero_rows_keep_names_and_exact_types() {
    let schema = Arc::new(Schema::new(vec![
        to_arrow("enum", &loams_house_types::parse("Enum8('yes'=1)").unwrap()).unwrap(),
    ]));
    assert_eq!(
        compact_json(&schema, &[]).unwrap(),
        b"[\"enum\"]\n[\"Enum8('yes' = 1)\"]\n"
    );
}
#[tokio::test]
async fn errors_keep_clickhouse_codes_and_unsupported_features_fail() {
    let catalog = MemoryCatalog::default();
    for (sql, code) in [
        ("SELECT * FROM missing", 60),
        ("SELECT 1 FORMAT CSV", 48),
        ("SELECT {x:UInt8}", 456),
        ("SELECT 1 SETTINGS max_threads=2", 115),
        ("CREATE TABLE t (x UInt8) ENGINE=Memory", 48),
    ] {
        assert_eq!(
            execute(sql, &[], "default", &catalog)
                .await
                .unwrap_err()
                .code,
            code,
            "{sql}"
        );
    }
}
async fn post(address: std::net::SocketAddr, path: &str, body: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream.write_all(format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    String::from_utf8(bytes).unwrap()
}
#[tokio::test]
async fn http_binds_parameters_and_reports_exception_headers() {
    let listener = loams_house_engine::http::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(loams_house_engine::http::serve(listener, None));
    let response = post(
        address,
        "/?query=SELECT%20%7Bx%3AUInt8%7D%20AS%20k&param_x=7",
        "",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("[\"UInt8\"]\n[7]"), "{response}");
    let response = post(address, "/", "SELECT * FROM missing").await;
    assert!(
        response.contains("x-clickhouse-exception-code: 60"),
        "{response}"
    );
    assert!(response.contains("Code: 60. DB::Exception:"), "{response}");
    let response = post(address, "/?max_threads=2", "SELECT 1").await;
    assert!(
        response.contains("x-clickhouse-exception-code: 115"),
        "{response}"
    );
    server.abort();
}
#[tokio::test]
async fn dev_server_refuses_public_interfaces() {
    assert!(
        loams_house_engine::http::bind("0.0.0.0:0".parse().unwrap())
            .await
            .is_err()
    );
}
#[test]
fn fixture_paths_cannot_escape_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    snapshot(dir.path());
    let file = dir.path().join("snapshot.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    value["tables"][0]["parquet"] = "/etc/passwd".into();
    std::fs::write(file, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(
        Snapshot::load(dir.path())
            .unwrap_err()
            .to_string()
            .contains("escapes")
    );
}
