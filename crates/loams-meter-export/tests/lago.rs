//! The Lago exporter against a fake Lago API (wiremock): window events,
//! differences for re-closed windows, retries and duplicates (plan ME1 Task
//! 26, D2214).

use std::sync::Arc;
use std::time::Duration;

use loams_meter_export::lago::{LagoConfig, LagoExporter, LagoWindow};
use loams_meter_export::{ExportError, MemLedger, Quantity, RetryPolicy, WindowRow, WindowSize};
use serde_json::Value;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const START: i64 = 1_800_000_000;

fn config(server: &MockServer) -> LagoConfig {
    let mut c = LagoConfig::new(server.uri(), "lago_test_key");
    c.retry = RetryPolicy {
        attempts: 3,
        base: Duration::from_millis(1),
        max: Duration::from_millis(5),
    };
    c
}

fn window(units: i64, run: u64, group: &str) -> LagoWindow {
    LagoWindow {
        window: WindowRow {
            meter: "llm_output_tokens".into(),
            subject: "org_42".into(),
            window_size: WindowSize::Hour,
            window_start: START,
            group_key: group.into(),
            value: Quantity::from_units(units),
            events: 1,
            close_run: run,
        },
        external_subscription_id: "sub_42".into(),
        code: "llm_output_tokens".into(),
    }
}

async fn accept(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/events/batch"))
        .and(header("authorization", "Bearer lago_test_key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"events":[]}"#))
        .mount(server)
        .await;
}

async fn events(server: &MockServer) -> Vec<Vec<Value>> {
    server
        .received_requests()
        .await
        .expect("recording")
        .iter()
        .map(|r| {
            let v: Value = serde_json::from_slice(&r.body).expect("JSON");
            v["events"].as_array().expect("events").clone()
        })
        .collect()
}

#[tokio::test]
async fn lago_sends_window_events() {
    let server = MockServer::start().await;
    accept(&server).await;
    let ex = LagoExporter::new(config(&server), Arc::new(MemLedger::new())).expect("exporter");
    let report = ex
        .export(&[window(500, 1, r#"{"model":"m-large"}"#)])
        .await
        .expect("export");
    assert_eq!(report.sent, 1);
    let e = &events(&server).await[0][0];
    assert_eq!(e["external_subscription_id"], "sub_42");
    assert_eq!(e["code"], "llm_output_tokens");
    assert_eq!(e["timestamp"], START);
    assert_eq!(e["properties"]["value"], "500");
    assert_eq!(
        e["properties"]["model"], "m-large",
        "group-by values ride along"
    );
    let id = e["transaction_id"].as_str().expect("id");
    assert!(
        id.starts_with(&format!("loams:llm_output_tokens:org_42:{START}:1:g")),
        "{id}"
    );
}

#[tokio::test]
async fn lago_reclosed_window_sends_the_difference() {
    let server = MockServer::start().await;
    accept(&server).await;
    let ex = LagoExporter::new(config(&server), Arc::new(MemLedger::new())).expect("exporter");
    ex.export(&[window(500, 1, "")]).await.expect("export");
    // A late event re-closes the window at 650: +150 under a new id.
    let r = ex.export(&[window(650, 2, "")]).await.expect("export");
    assert_eq!((r.sent, r.adjustments), (1, 1));
    // Re-closed lower (a duplicate removed by FINAL): −50.
    ex.export(&[window(600, 3, "")]).await.expect("export");
    // Unchanged: nothing.
    let r = ex.export(&[window(600, 3, "")]).await.expect("export");
    assert_eq!((r.sent, r.unchanged), (0, 1));

    let sent: Vec<(String, String)> = events(&server)
        .await
        .iter()
        .flatten()
        .map(|e| {
            (
                e["transaction_id"].as_str().expect("id").to_owned(),
                e["properties"]["value"].as_str().expect("value").to_owned(),
            )
        })
        .collect();
    let id = |run: u64| format!("loams:llm_output_tokens:org_42:{START}:{run}");
    assert_eq!(
        sent,
        vec![
            (id(1), "500".into()),
            (id(2), "150".into()),
            (id(3), "-50".into())
        ],
        "the events add up to the window's value"
    );
}

#[tokio::test]
async fn lago_unaccepted_event_is_resent_unchanged_first() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/events/batch"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    let ex = LagoExporter::new(config(&server), Arc::new(MemLedger::new())).expect("exporter");
    assert!(matches!(
        ex.export(&[window(500, 1, "")]).await,
        Err(ExportError::Exhausted { .. })
    ));
    let failed_id = events(&server).await[0][0]["transaction_id"].clone();

    // Lago is back and the window re-closed meanwhile: this run resends the
    // planned event as it was; the difference follows on the next run.
    server.reset().await;
    accept(&server).await;
    let r = ex.export(&[window(700, 2, "")]).await.expect("export");
    assert_eq!((r.sent, r.resent), (1, 1));
    let e = &events(&server).await[0][0];
    assert_eq!(e["transaction_id"], failed_id);
    assert_eq!(e["properties"]["value"], "500");
    server.reset().await;
    accept(&server).await;
    ex.export(&[window(700, 2, "")]).await.expect("export");
    let e = &events(&server).await[0][0];
    assert_eq!(e["properties"]["value"], "200");
}

#[tokio::test]
async fn lago_duplicate_transaction_counts_as_accepted() {
    let server = MockServer::start().await;
    let dup = r#"{"status":422,"error":"Unprocessable Entity","code":"validation_errors","error_details":{"transaction_id":["value_already_exist"]}}"#;
    // The batch is refused because one event is known; alone, that one is
    // refused again and the other is accepted.
    Mock::given(method("POST"))
        .and(path("/api/v1/events/batch"))
        .and(wiremock::matchers::body_string_contains(":org_42:"))
        .and(wiremock::matchers::body_string_contains(":org_7:"))
        .respond_with(ResponseTemplate::new(422).set_body_string(dup))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/events/batch"))
        .and(wiremock::matchers::body_string_contains(":org_42:"))
        .respond_with(ResponseTemplate::new(422).set_body_string(dup))
        .mount(&server)
        .await;
    accept(&server).await;
    let ledger = Arc::new(MemLedger::new());
    let ex = LagoExporter::new(config(&server), ledger.clone()).expect("exporter");
    let mut other = window(9, 1, "");
    other.window.subject = "org_7".into();
    other.external_subscription_id = "sub_7".into();
    let r = ex
        .export(&[window(500, 1, ""), other])
        .await
        .expect("export");
    assert_eq!(r.sent, 2);
    assert_eq!(
        events(&server).await.len(),
        3,
        "the batch, then each event alone"
    );
    assert!(ledger.entries().await.iter().all(|(_, e)| e.sent));
}
