//! The SQL Gateway client against a mock gateway (httpmock), with the
//! request and answer shapes of the Flink 2.x SQL Gateway REST API.

use std::collections::BTreeMap;
use std::time::Duration;

use httpmock::Method::{DELETE, GET, POST};
use httpmock::MockServer;
use loams_flink::FlinkError;
use loams_flink::sql_gateway::{
    ApiVersion, DeployScript, ExecuteStatement, FetchLimits, OpenSession, OperationHandle,
    OperationStatus, RowKind, SessionHandle, SqlGatewayClient,
};
use serde_json::json;

const S: &str = "2b0b1c7e-5d3c-4a1e-9f53-6f2b4c8a9d10";
const O: &str = "8c2f0a43-1d7e-4b6a-a3f4-0e9d5c7b2a61";

fn client(server: &MockServer, v: ApiVersion) -> SqlGatewayClient {
    SqlGatewayClient::new(&server.base_url(), v).expect("a client for the mock server")
}

#[tokio::test]
async fn negotiate_picks_the_highest_supported_version() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET).path("/api_versions");
            then.status(200)
                .json_body(json!({"versions": ["V1", "V2", "V3", "V4"]}));
        })
        .await;
    let c = client(&server, ApiVersion::V2).negotiate().await.unwrap();
    assert_eq!(c.version(), ApiVersion::V4);
}

#[tokio::test]
async fn negotiate_refuses_a_v1_only_gateway() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET).path("/api_versions");
            then.status(200).json_body(json!({"versions": ["V1"]}));
        })
        .await;
    let err = client(&server, ApiVersion::V3)
        .negotiate()
        .await
        .unwrap_err();
    assert!(matches!(err, FlinkError::Unsupported(_)), "{err:?}");
}

#[tokio::test]
async fn sessions_open_configure_heartbeat_and_close() {
    let server = MockServer::start_async().await;
    let open = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v3/sessions")
                .json_body(json!({"sessionName": "t1", "properties": {"pipeline.name": "orders"}}));
            then.status(200).json_body(json!({"sessionHandle": S}));
        })
        .await;
    let configure = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(format!("/v3/sessions/{S}/configure-session"))
                .json_body(json!({"statement": "USE CATALOG loams_lake"}));
            then.status(200).json_body(json!({}));
        })
        .await;
    let heartbeat = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(format!("/v3/sessions/{S}/heartbeat"));
            then.status(200).body("");
        })
        .await;
    let config = server
        .mock_async(|when, then| {
            when.method(GET).path(format!("/v3/sessions/{S}"));
            then.status(200)
                .json_body(json!({"properties": {"pipeline.name": "orders"}}));
        })
        .await;
    let close = server
        .mock_async(|when, then| {
            when.method(DELETE).path(format!("/v3/sessions/{S}"));
            then.status(200).json_body(json!({"status": "CLOSED"}));
        })
        .await;

    let c = client(&server, ApiVersion::V3);
    let s = c
        .open_session(&OpenSession {
            session_name: Some("t1".into()),
            properties: BTreeMap::from([("pipeline.name".to_owned(), "orders".to_owned())]),
        })
        .await
        .unwrap();
    assert_eq!(s.as_str(), S);
    c.configure_session(&s, "USE CATALOG loams_lake", None)
        .await
        .unwrap();
    c.heartbeat(&s).await.unwrap();
    assert_eq!(
        c.session_config(&s).await.unwrap()["pipeline.name"],
        "orders"
    );
    assert_eq!(c.close_session(&s).await.unwrap(), "CLOSED");
    open.assert_async().await;
    configure.assert_async().await;
    heartbeat.assert_async().await;
    config.assert_async().await;
    close.assert_async().await;
}

#[tokio::test]
async fn complete_statement_sends_its_arguments_on_the_get() {
    let server = MockServer::start_async().await;
    let m = server
        .mock_async(|when, then| {
            when.method(GET)
                .path(format!("/v3/sessions/{S}/complete-statement"))
                .json_body(json!({"statement": "SEL", "position": 3}));
            then.status(200)
                .json_body(json!({"candidates": ["SELECT"]}));
        })
        .await;
    let c = client(&server, ApiVersion::V3);
    let s = SessionHandle::new(S).unwrap();
    assert_eq!(
        c.complete_statement(&s, "SEL", 3).await.unwrap(),
        ["SELECT"]
    );
    m.assert_async().await;
}

#[tokio::test]
async fn results_are_paged_through_next_result_uri() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(POST)
                .path(format!("/v3/sessions/{S}/statements"))
                .json_body(json!({"statement": "SELECT * FROM t"}));
            then.status(200).json_body(json!({"operationHandle": O}));
        })
        .await;
    let page0 = server
        .mock_async(|when, then| {
            when.method(GET)
                .path(format!("/v3/sessions/{S}/operations/{O}/result/0"))
                .query_param("rowFormat", "JSON");
            then.status(200).json_body(json!({
                "resultType": "NOT_READY",
                "nextResultUri": format!("/v3/sessions/{S}/operations/{O}/result/1?rowFormat=JSON")
            }));
        })
        .await;
    let page1 = server
        .mock_async(|when, then| {
            when.method(GET)
                .path(format!("/v3/sessions/{S}/operations/{O}/result/1"))
                .query_param("rowFormat", "JSON");
            then.status(200).json_body(json!({
                "resultType": "PAYLOAD",
                "isQueryResult": true,
                "resultKind": "SUCCESS_WITH_CONTENT",
                "results": {
                    "columns": [{"name": "id", "logicalType": {"type": "BIGINT", "nullable": false}}],
                    "rowFormat": "JSON",
                    "data": [{"kind": "INSERT", "fields": [1]}, {"kind": "INSERT", "fields": [2]}]
                },
                "nextResultUri": format!("/v3/sessions/{S}/operations/{O}/result/2?rowFormat=JSON")
            }));
        })
        .await;
    let page2 = server
        .mock_async(|when, then| {
            when.method(GET)
                .path(format!("/v3/sessions/{S}/operations/{O}/result/2"));
            then.status(200).json_body(json!({
                "resultType": "PAYLOAD",
                "results": {"columns": [], "data": [{"kind": "DELETE", "fields": [1]}]},
                "nextResultUri": format!("/v3/sessions/{S}/operations/{O}/result/3?rowFormat=JSON")
            }));
        })
        .await;
    let eos = server
        .mock_async(|when, then| {
            when.method(GET)
                .path(format!("/v3/sessions/{S}/operations/{O}/result/3"));
            then.status(200).json_body(json!({"resultType": "EOS"}));
        })
        .await;
    let close = server
        .mock_async(|when, then| {
            when.method(DELETE)
                .path(format!("/v3/sessions/{S}/operations/{O}/close"));
            then.status(200).json_body(json!({"status": "CLOSED"}));
        })
        .await;

    let c = client(&server, ApiVersion::V3);
    let s = SessionHandle::new(S).unwrap();
    let result = c
        .execute_and_collect(
            &s,
            &ExecuteStatement::new("SELECT * FROM t"),
            FetchLimits {
                poll: Duration::from_millis(1),
                ..FetchLimits::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(result.columns[0].name, "id");
    assert_eq!(result.rows.len(), 3);
    assert_eq!(result.rows[2].kind, RowKind::Delete);
    assert!(!result.truncated);
    assert_eq!(result.is_query_result, Some(true));
    for m in [&page0, &page1, &page2, &eos, &close] {
        m.assert_async().await;
    }
}

#[tokio::test]
async fn fetch_all_stops_at_max_rows_for_streams() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET)
                .path(format!("/v3/sessions/{S}/operations/{O}/result/0"));
            then.status(200).json_body(json!({
                "resultType": "PAYLOAD",
                "results": {"columns": [], "data": [
                    {"kind": "INSERT", "fields": [1]},
                    {"kind": "INSERT", "fields": [2]},
                    {"kind": "INSERT", "fields": [3]}
                ]},
                "nextResultUri": format!("/v3/sessions/{S}/operations/{O}/result/1?rowFormat=JSON")
            }));
        })
        .await;
    let c = client(&server, ApiVersion::V3);
    let result = c
        .fetch_all(
            &SessionHandle::new(S).unwrap(),
            &OperationHandle::new(O).unwrap(),
            FetchLimits {
                max_rows: 2,
                ..FetchLimits::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 2);
    assert!(result.truncated);
}

#[tokio::test]
async fn statement_errors_carry_flinks_message() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET)
                .path(format!("/v3/sessions/{S}/operations/{O}/result/0"));
            then.status(500).json_body(json!({"errors": [
                "Internal server error.",
                "org.apache.flink.table.api.ValidationException: Object 'nope' not found\n\tat ..."
            ]}));
        })
        .await;
    let c = client(&server, ApiVersion::V3);
    let err = c
        .fetch_all(
            &SessionHandle::new(S).unwrap(),
            &OperationHandle::new(O).unwrap(),
            FetchLimits::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(500));
    assert!(err.to_string().contains("Object 'nope' not found"), "{err}");
}

#[tokio::test]
async fn operations_status_cancel_and_close() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET)
                .path(format!("/v3/sessions/{S}/operations/{O}/status"));
            then.status(200).json_body(json!({"status": "RUNNING"}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method(POST)
                .path(format!("/v3/sessions/{S}/operations/{O}/cancel"));
            then.status(200).json_body(json!({"status": "CANCELED"}));
        })
        .await;
    let c = client(&server, ApiVersion::V3);
    let s = SessionHandle::new(S).unwrap();
    let o = OperationHandle::new(O).unwrap();
    assert_eq!(
        c.operation_status(&s, &o).await.unwrap(),
        OperationStatus::Running
    );
    assert_eq!(
        c.cancel_operation(&s, &o).await.unwrap(),
        OperationStatus::Canceled
    );
}

#[tokio::test]
async fn deploy_script_uses_v4_and_returns_the_cluster_id() {
    let server = MockServer::start_async().await;
    let m = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(format!("/v4/sessions/{S}/scripts"))
                .json_body(json!({
                    "scriptUri": "s3://flink/flink/t1/scripts/etl.sql",
                    "executionConfig": {"pipeline.name": "etl"}
                }));
            then.status(200)
                .json_body(json!({"clusterID": "flink-app-etl"}));
        })
        .await;
    let c = client(&server, ApiVersion::V4);
    let id = c
        .deploy_script(
            &SessionHandle::new(S).unwrap(),
            &DeployScript {
                script: None,
                script_uri: Some("s3://flink/flink/t1/scripts/etl.sql".into()),
                execution_config: BTreeMap::from([("pipeline.name".to_owned(), "etl".to_owned())]),
            },
        )
        .await
        .unwrap();
    assert_eq!(id, "flink-app-etl");
    m.assert_async().await;
}

#[tokio::test]
async fn v2_sends_row_format_and_info_works() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET).path("/v2/info");
            then.status(200)
                .json_body(json!({"productName": "Apache Flink", "version": "2.2.1"}));
        })
        .await;
    let c = client(&server, ApiVersion::V2);
    let info = c.info().await.unwrap();
    assert_eq!(info.version, "2.2.1");
}
