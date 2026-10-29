//! The OpenMeter exporter against a fake OpenMeter (wiremock): batches,
//! `loamslease` stripped, retries with the same ids, and the cursor (plan
//! ME1 Task 25, D2213).

use std::sync::Arc;
use std::time::Duration;

use loams_meter_export::openmeter::{EventRow, OpenMeterConfig, OpenMeterExporter};
use loams_meter_export::{ExportCursors, ExportError, MemCursors, RetryPolicy};
use serde_json::Value;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(server: &MockServer) -> OpenMeterConfig {
    let mut c = OpenMeterConfig::new(server.uri());
    c.token = Some("om_test_token".into());
    c.retry = RetryPolicy {
        attempts: 3,
        base: Duration::from_millis(1),
        max: Duration::from_millis(5),
    };
    c
}

fn row(partition: u32, offset: u64, ty: &str) -> EventRow {
    EventRow {
        subject: "org_42".into(),
        r#type: ty.into(),
        source: "loams://prod-eu1/house/node-3".into(),
        id: format!("q-{partition}-{offset}"),
        time: "2026-10-19T09:14:03.218734Z".into(),
        tenantid: "42/7".into(),
        lease: "l9k2m4q8x1".into(),
        data: format!(r#"{{"cpu_ms":{offset}}}"#),
        stream_partition: partition,
        stream_offset: offset,
    }
}

const QUERY: &str = "io.loams.dev.usage.house.query.v1";

async fn accept(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/events"))
        .and(header("content-type", "application/cloudevents-batch+json"))
        .and(header("authorization", "Bearer om_test_token"))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
        .await;
}

async fn batches(server: &MockServer) -> Vec<Vec<Value>> {
    server
        .received_requests()
        .await
        .expect("recording")
        .iter()
        .map(|r| serde_json::from_slice::<Vec<Value>>(&r.body).expect("a JSON batch"))
        .collect()
}

#[tokio::test]
async fn openmeter_strips_loamslease() {
    let server = MockServer::start().await;
    accept(&server).await;
    let ex =
        OpenMeterExporter::new(config(&server), Arc::new(MemCursors::new())).expect("exporter");
    let report = ex.export(&[row(0, 0, QUERY)]).await.expect("export");
    assert_eq!(report.sent, 1);
    let sent = batches(&server).await;
    let ce = &sent[0][0];
    assert!(ce.get("loamslease").is_none(), "{ce}");
    assert!(!ce.to_string().contains("l9k2m4q8x1"));
    // Everything else as the producer sent it.
    assert_eq!(ce["id"], "q-0-0");
    assert_eq!(ce["source"], "loams://prod-eu1/house/node-3");
    assert_eq!(ce["type"], QUERY);
    assert_eq!(ce["subject"], "org_42");
    assert_eq!(ce["time"], "2026-10-19T09:14:03.218734Z");
    assert_eq!(ce["tenantid"], "42/7");
    assert_eq!(ce["data"]["cpu_ms"], 0);
}

#[tokio::test]
async fn openmeter_retry_resends_the_same_ids_and_cursor_waits_for_the_accept() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/events"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    accept(&server).await;
    let cursors = Arc::new(MemCursors::new());
    let ex = OpenMeterExporter::new(config(&server), cursors.clone()).expect("exporter");
    let rows = [row(1, 5, QUERY), row(1, 6, QUERY)];
    ex.export(&rows).await.expect("export");
    let sent = batches(&server).await;
    assert_eq!(sent.len(), 2, "one failure, one retry");
    assert_eq!(sent[0], sent[1], "the retry is the same batch, same ids");
    assert_eq!(cursors.get("openmeter", 1).await.expect("cursor"), Some(7));

    // Exported already: nothing is sent again.
    server.reset().await;
    accept(&server).await;
    let report = ex.export(&rows).await.expect("export");
    assert_eq!((report.sent, report.skipped), (0, 2));
    assert!(batches(&server).await.is_empty());

    // OpenMeter down for the whole run: the cursor does not move.
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/events"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    assert!(matches!(
        ex.export(&[row(1, 7, QUERY)]).await,
        Err(ExportError::Exhausted { .. })
    ));
    assert_eq!(cursors.get("openmeter", 1).await.expect("cursor"), Some(7));
}

#[tokio::test]
async fn openmeter_batches_per_partition_and_filters_types() {
    let server = MockServer::start().await;
    accept(&server).await;
    let cursors = Arc::new(MemCursors::new());
    let mut cfg = config(&server);
    cfg.batch_size = 2;
    let ex = OpenMeterExporter::new(cfg, cursors.clone()).expect("exporter");
    let rows = [
        row(0, 0, QUERY),
        row(0, 1, "io.loams.dev.meter.threshold.v1"),
        row(0, 2, QUERY),
        row(0, 3, QUERY),
        row(2, 0, QUERY),
        row(0, 4, QUERY),
    ];
    let report = ex.export(&rows).await.expect("export");
    assert_eq!((report.sent, report.filtered, report.batches), (5, 1, 3));
    let ids: Vec<Vec<String>> = batches(&server)
        .await
        .iter()
        .map(|b| {
            b.iter()
                .map(|ce| ce["id"].as_str().expect("id").to_owned())
                .collect()
        })
        .collect();
    assert_eq!(
        ids,
        vec![
            vec!["q-0-0".to_owned(), "q-0-2".to_owned()],
            vec!["q-0-3".to_owned(), "q-0-4".to_owned()],
            vec!["q-2-0".to_owned()],
        ],
        "partition and offset order; the threshold event is not exported"
    );
    assert_eq!(cursors.get("openmeter", 0).await.expect("cursor"), Some(5));
    assert_eq!(cursors.get("openmeter", 2).await.expect("cursor"), Some(1));
}

#[tokio::test]
async fn openmeter_refusal_stops_with_the_problem_detail() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/events"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"type":"about:blank","title":"Bad Request","detail":"subject is required"}"#,
        ))
        .mount(&server)
        .await;
    let cursors = Arc::new(MemCursors::new());
    let ex = OpenMeterExporter::new(config(&server), cursors.clone()).expect("exporter");
    match ex.export(&[row(0, 0, QUERY)]).await {
        Err(ExportError::Rejected {
            status: 400,
            message,
            ..
        }) => {
            assert_eq!(message, "Bad Request: subject is required");
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
    assert_eq!(batches(&server).await.len(), 1, "a 400 is not retried");
    assert_eq!(cursors.get("openmeter", 0).await.expect("cursor"), None);
}
