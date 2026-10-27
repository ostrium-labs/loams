//! The CloudEvents webhook exporter against a fake receiver (wiremock):
//! signed batches, the same delivery id on a retry, and re-closed windows
//! (plan ME1 Task 26, D2215).

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use loams_meter_export::webhook::{WebhookConfig, WebhookExporter};
use loams_meter_export::{MemLedger, Quantity, RetryPolicy, WindowRow, WindowSize};
use serde_json::Value;
use sha2::Sha256;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const KEY: &[u8] = b"loams-webhook-test-key";
const NOW: i64 = 1_800_010_000;

fn config(server: &MockServer) -> WebhookConfig {
    let mut c = WebhookConfig::new(
        format!("{}/billing/loams", server.uri()),
        format!("whsec_{}", STANDARD.encode(KEY)),
        "loams://test/meter",
    );
    c.retry = RetryPolicy {
        attempts: 3,
        base: Duration::from_millis(1),
        max: Duration::from_millis(5),
    };
    c
}

fn w(start_hours: i64, units: i64, run: u64) -> WindowRow {
    WindowRow {
        meter: "house_cpu_ms".into(),
        subject: "org_42".into(),
        window_size: WindowSize::Hour,
        window_start: 1_800_000_000 + start_hours * 3600,
        group_key: String::new(),
        value: Quantity::from_units(units),
        events: 1,
        close_run: run,
    }
}

fn header_of<'a>(r: &'a Request, name: &str) -> &'a str {
    r.headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

/// Checks a delivery as a Standard Webhooks receiver does.
fn verify(r: &Request) -> bool {
    let id = header_of(r, "webhook-id");
    let ts = header_of(r, "webhook-timestamp");
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(KEY).expect("key");
    mac.update(format!("{id}.{ts}.").as_bytes());
    mac.update(&r.body);
    let want = format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()));
    header_of(r, "webhook-signature") == want
}

async fn accept(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/billing/loams"))
        .and(header("content-type", "application/cloudevents-batch+json"))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
        .await;
}

#[tokio::test]
async fn webhook_sends_signed_cloudevents_and_retries_with_the_same_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/billing/loams"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    accept(&server).await;
    let ex = WebhookExporter::new(config(&server), Arc::new(MemLedger::new())).expect("exporter");
    let report = ex
        .export(&[w(0, 10, 1), w(1, 20, 1)], NOW)
        .await
        .expect("export");
    assert_eq!((report.sent, report.batches), (2, 1));

    let reqs = server.received_requests().await.expect("recording");
    assert_eq!(reqs.len(), 2, "one failure, one retry");
    assert!(reqs.iter().all(verify), "every delivery is signed");
    assert_eq!(
        header_of(&reqs[0], "webhook-id"),
        header_of(&reqs[1], "webhook-id")
    );
    assert_eq!(header_of(&reqs[0], "webhook-timestamp"), NOW.to_string());
    let batch: Vec<Value> = serde_json::from_slice(&reqs[1].body).expect("batch");
    assert_eq!(batch[0]["type"], "io.loams.dev.meter.window.closed.v1");
    assert_eq!(batch[0]["source"], "loams://test/meter");
    assert_eq!(batch[0]["id"], "house_cpu_ms/org_42/hour/1800000000/1");
    assert_eq!(batch[1]["data"]["value"], "20");
}

#[tokio::test]
async fn webhook_sends_a_window_again_only_when_reclosed() {
    let server = MockServer::start().await;
    accept(&server).await;
    let ex = WebhookExporter::new(config(&server), Arc::new(MemLedger::new())).expect("exporter");
    ex.export(&[w(0, 10, 1)], NOW).await.expect("export");
    let r = ex.export(&[w(0, 10, 1)], NOW).await.expect("export");
    assert_eq!((r.sent, r.unchanged), (0, 1));
    ex.export(&[w(0, 12, 2)], NOW).await.expect("export");

    let ids: Vec<String> = server
        .received_requests()
        .await
        .expect("recording")
        .iter()
        .flat_map(|r| serde_json::from_slice::<Vec<Value>>(&r.body).expect("batch"))
        .map(|e| e["id"].as_str().expect("id").to_owned())
        .collect();
    assert_eq!(
        ids,
        vec![
            "house_cpu_ms/org_42/hour/1800000000/1".to_owned(),
            "house_cpu_ms/org_42/hour/1800000000/2".to_owned(),
        ]
    );
}
