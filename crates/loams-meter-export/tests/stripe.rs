//! The Stripe exporter against a fake Stripe (wiremock): meter events per
//! closed hour, identifiers across retries and re-closed hours, and payment
//! state from webhooks and the daily poll (plan ME1 Task 24, D2211, D2212).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use hmac::{Hmac, KeyInit, Mac};
use loams_meter_export::stripe::payment::{
    PaymentState, PaymentStateSink, StripePayments, WebhookOutcome, WebhookVerifier,
};
use loams_meter_export::stripe::{HourUsage, PeriodExport, StripeConfig, StripeExporter};
use loams_meter_export::{ExportError, ExportLedger, MemLedger, Quantity, RetryPolicy};
use sha2::Sha256;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// A whole UTC hour.
const P: i64 = 1_800_000_000;
const H: i64 = 3600;

fn config(server: &MockServer) -> StripeConfig {
    let mut c = StripeConfig::new("sk_test_loams");
    c.api_base = server.uri();
    c.retry = RetryPolicy {
        attempts: 3,
        base: Duration::from_millis(1),
        max: Duration::from_millis(5),
    };
    c
}

fn hour(n: i64, units: i64, run: u64) -> HourUsage {
    HourUsage {
        hour_start: P + n * H,
        value: Quantity::from_units(units),
        close_run: run,
    }
}

fn period(included: i64, hours: Vec<HourUsage>) -> PeriodExport {
    PeriodExport {
        event_name: "house_cpu_ms".into(),
        subject: "org_42".into(),
        stripe_customer_id: "cus_Loams42".into(),
        period_start: P,
        included: Quantity::from_units(included),
        hours,
    }
}

fn form(r: &Request) -> HashMap<String, String> {
    url::form_urlencoded::parse(&r.body).into_owned().collect()
}

async fn meter_events(server: &MockServer) -> Vec<HashMap<String, String>> {
    server
        .received_requests()
        .await
        .expect("recording")
        .iter()
        .filter(|r| r.url.path() == "/v1/billing/meter_events")
        .map(form)
        .collect()
}

async fn accept(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/billing/meter_events"))
        .and(header("authorization", "Bearer sk_test_loams"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"object":"billing.meter_event"}"#),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn stripe_identifier_stable_across_retries() {
    let server = MockServer::start().await;
    // A server error first, then success: the retry sends the same event.
    Mock::given(method("POST"))
        .and(path("/v1/billing/meter_events"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    accept(&server).await;
    let ledger = Arc::new(MemLedger::new());
    let ex = StripeExporter::new(config(&server), ledger.clone()).expect("exporter");

    let p = period(100, vec![hour(0, 150, 1)]);
    let report = ex.export_period(&p).await.expect("export");
    assert_eq!(report.sent, 1);
    let reqs = server.received_requests().await.expect("recording");
    assert_eq!(reqs.len(), 2, "one failure, one retry");
    let ids: Vec<String> = reqs.iter().map(|r| form(r)["identifier"].clone()).collect();
    assert_eq!(ids[0], format!("loams:house_cpu_ms:org_42:{P}:1"));
    assert_eq!(ids[0], ids[1], "the retry keeps the identifier");
    for r in &reqs {
        assert_eq!(
            r.headers
                .get("idempotency-key")
                .and_then(|v| v.to_str().ok()),
            Some(ids[0].as_str()),
            "and the Idempotency-Key"
        );
    }

    // A run whose every attempt fails leaves the planned send in the ledger;
    // the next run sends the same identifier, not a new one.
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/v1/billing/meter_events"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let p = period(100, vec![hour(0, 150, 1), hour(1, 10, 1)]);
    match ex.export_period(&p).await {
        Err(ExportError::Exhausted { attempts: 3, .. }) => {}
        other => panic!("expected Exhausted, got {other:?}"),
    }
    let failed: Vec<String> = meter_events(&server)
        .await
        .iter()
        .map(|f| f["identifier"].clone())
        .collect();
    assert_eq!(failed.len(), 3);
    assert!(
        failed
            .iter()
            .all(|i| *i == format!("loams:house_cpu_ms:org_42:{}:1", P + H))
    );

    server.reset().await;
    accept(&server).await;
    let report = ex.export_period(&p).await.expect("export");
    assert_eq!(report.sent, 1, "only the hour that was not accepted");
    let sent = meter_events(&server).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0]["identifier"], failed[0],
        "the same identifier after the failed run"
    );
    assert_eq!(sent[0]["payload[value]"], "10");

    // Nothing changed: nothing is sent.
    server.reset().await;
    accept(&server).await;
    let report = ex.export_period(&p).await.expect("export");
    assert_eq!((report.sent, report.unchanged), (0, 2));
    assert!(meter_events(&server).await.is_empty());
    assert!(ledger.entries().await.iter().all(|(_, e)| e.sent));
}

#[tokio::test]
async fn reclosed_window_sends_adjustment() {
    let server = MockServer::start().await;
    accept(&server).await;
    let ex = StripeExporter::new(config(&server), Arc::new(MemLedger::new())).expect("exporter");

    // 100 included; 50 then 80: hour 1 goes 30 over.
    ex.export_period(&period(100, vec![hour(0, 50, 1), hour(1, 80, 1)]))
        .await
        .expect("export");
    // Hour 0 re-closes (a late event) at 70: hour 1's overage becomes 50,
    // sent under the new revision, which overwrites the hour in Stripe.
    ex.export_period(&period(100, vec![hour(0, 70, 2), hour(1, 80, 1)]))
        .await
        .expect("export");
    // Hour 0 re-closes at 10: hour 1 is no longer over, so 0 overwrites 50.
    ex.export_period(&period(100, vec![hour(0, 10, 3), hour(1, 80, 1)]))
        .await
        .expect("export");

    let sent = meter_events(&server).await;
    let got: Vec<(String, String, String)> = sent
        .iter()
        .map(|f| {
            (
                f["identifier"].clone(),
                f["payload[value]"].clone(),
                f["timestamp"].clone(),
            )
        })
        .collect();
    let h1 = (P + H).to_string();
    let want = |run: u64, value: &str| {
        (
            format!("loams:house_cpu_ms:org_42:{h1}:{run}"),
            value.to_owned(),
            h1.clone(),
        )
    };
    assert_eq!(got, vec![want(1, "30"), want(2, "50"), want(3, "0")]);
    for f in &sent {
        assert_eq!(f["event_name"], "house_cpu_ms");
        assert_eq!(f["payload[stripe_customer_id]"], "cus_Loams42");
    }
}

#[tokio::test]
async fn same_revision_with_a_new_value_gets_a_new_identifier() {
    let server = MockServer::start().await;
    accept(&server).await;
    let ex = StripeExporter::new(config(&server), Arc::new(MemLedger::new())).expect("exporter");
    ex.export_period(&period(100, vec![hour(0, 150, 4)]))
        .await
        .expect("export");
    // The included amount changed (a grant), the windows did not.
    ex.export_period(&period(120, vec![hour(0, 150, 4)]))
        .await
        .expect("export");
    let ids: Vec<String> = meter_events(&server)
        .await
        .iter()
        .map(|f| f["identifier"].clone())
        .collect();
    assert_eq!(
        ids,
        vec![
            format!("loams:house_cpu_ms:org_42:{P}:4"),
            format!("loams:house_cpu_ms:org_42:{P}:4-1"),
        ]
    );
}

#[tokio::test]
async fn refused_request_is_not_retried_and_off_hour_period_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/billing/meter_events"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"code":"parameter_invalid","message":"timestamp too old"}}"#,
        ))
        .mount(&server)
        .await;
    let ledger = Arc::new(MemLedger::new());
    let ex = StripeExporter::new(config(&server), ledger.clone()).expect("exporter");
    match ex.export_period(&period(0, vec![hour(0, 5, 1)])).await {
        Err(ExportError::Rejected {
            status: 400,
            message,
            ..
        }) => {
            assert!(message.contains("timestamp too old"), "{message}");
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
    assert_eq!(meter_events(&server).await.len(), 1, "a 400 is not retried");
    assert!(!ledger.entries().await[0].1.sent);

    server.reset().await;
    let mut off = period(0, vec![hour(0, 5, 1)]);
    off.period_start = P + 60;
    assert!(matches!(
        ex.export_period(&off).await,
        Err(ExportError::OffHourPeriod { .. })
    ));
    assert!(meter_events(&server).await.is_empty());
}

// --- payment state ------------------------------------------------------

#[derive(Debug, Default)]
struct RecordingSink(Mutex<Vec<(String, PaymentState)>>);

impl RecordingSink {
    fn calls(&self) -> Vec<(String, PaymentState)> {
        self.0.lock().expect("lock").clone()
    }
}

#[async_trait]
impl PaymentStateSink for RecordingSink {
    async fn set_payment_state(
        &self,
        customer: &str,
        state: PaymentState,
    ) -> Result<(), ExportError> {
        self.0
            .lock()
            .expect("lock")
            .push((customer.to_owned(), state));
        Ok(())
    }
}

const SECRET: &str = "whsec_loams_test";

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
}

fn sign(body: &str, t: i64) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(SECRET.as_bytes()).expect("key");
    mac.update(format!("{t}.{body}").as_bytes());
    format!("t={t},v1={}", hex::encode(mac.finalize().into_bytes()))
}

async fn payments(server: &MockServer) -> (StripePayments, Arc<RecordingSink>) {
    let sink = Arc::new(RecordingSink::default());
    let p = StripePayments::new(
        config(server),
        WebhookVerifier::new(vec![SECRET.into()]).expect("verifier"),
        sink.clone(),
    )
    .expect("payments");
    (p, sink)
}

async fn customer(server: &MockServer, id: &str, default_pm: Option<&str>) {
    let body = serde_json::json!({
        "id": id, "object": "customer",
        "invoice_settings": { "default_payment_method": default_pm },
        "default_source": null,
    });
    Mock::given(method("GET"))
        .and(path(format!("/v1/customers/{id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn webhook_sets_card_from_the_customer_default() {
    let server = MockServer::start().await;
    customer(&server, "cus_A1", Some("pm_1")).await;
    let (p, sink) = payments(&server).await;
    // A payment method body carries card fields: they are not read.
    let body = r#"{"id":"evt_1","type":"payment_method.attached","data":{"object":{"id":"pm_1","object":"payment_method","customer":"cus_A1","card":{"brand":"visa","last4":"4242","exp_month":12,"exp_year":2030}}}}"#;
    let t = now();
    let out = p
        .handle_webhook(&sign(body, t), body.as_bytes(), t)
        .await
        .expect("handled");
    assert_eq!(
        out,
        WebhookOutcome::Updated {
            stripe_customer_id: "cus_A1".into(),
            state: PaymentState::Card
        }
    );
    assert_eq!(
        sink.calls(),
        vec![("cus_A1".to_owned(), PaymentState::Card)]
    );
    // Only the customer was read: no payment method listing.
    let paths: Vec<String> = server
        .received_requests()
        .await
        .expect("recording")
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert_eq!(paths, vec!["/v1/customers/cus_A1"]);
}

#[tokio::test]
async fn webhook_detach_reads_the_previous_customer_and_sets_none() {
    let server = MockServer::start().await;
    customer(&server, "cus_B2", None).await;
    let (p, sink) = payments(&server).await;
    let body = r#"{"type":"payment_method.detached","data":{"object":{"id":"pm_2","customer":null},"previous_attributes":{"customer":"cus_B2"}}}"#;
    let t = now();
    p.handle_webhook(&sign(body, t), body.as_bytes(), t)
        .await
        .expect("handled");
    assert_eq!(
        sink.calls(),
        vec![("cus_B2".to_owned(), PaymentState::None)]
    );
}

#[tokio::test]
async fn webhook_with_a_bad_signature_or_old_timestamp_sets_nothing() {
    let server = MockServer::start().await;
    customer(&server, "cus_A1", Some("pm_1")).await;
    let (p, sink) = payments(&server).await;
    let body = r#"{"type":"customer.updated","data":{"object":{"id":"cus_A1"}}}"#;
    let t = now();
    let forged = sign(
        r#"{"type":"customer.updated","data":{"object":{"id":"cus_OTHER"}}}"#,
        t,
    );
    assert!(matches!(
        p.handle_webhook(&forged, body.as_bytes(), t).await,
        Err(ExportError::Webhook(_))
    ));
    assert!(matches!(
        p.handle_webhook(&sign(body, t - 3600), body.as_bytes(), t)
            .await,
        Err(ExportError::Webhook(_))
    ));
    assert!(sink.calls().is_empty());
    assert!(
        server
            .received_requests()
            .await
            .expect("recording")
            .is_empty()
    );

    // An event type the exporter does not use is verified and ignored.
    let other = r#"{"type":"invoice.paid","data":{"object":{"id":"in_1","customer":"cus_A1"}}}"#;
    assert_eq!(
        p.handle_webhook(&sign(other, t), other.as_bytes(), t)
            .await
            .expect("handled"),
        WebhookOutcome::Ignored {
            event_type: "invoice.paid".into()
        }
    );
    assert!(sink.calls().is_empty());
}

#[tokio::test]
async fn daily_poll_sets_every_customer() {
    let server = MockServer::start().await;
    customer(&server, "cus_A1", Some("pm_1")).await;
    customer(&server, "cus_B2", None).await;
    Mock::given(method("GET"))
        .and(path("/v1/customers/cus_GONE"))
        .respond_with(ResponseTemplate::new(404).set_body_string(
            r#"{"error":{"code":"resource_missing","message":"No such customer"}}"#,
        ))
        .mount(&server)
        .await;
    let (p, sink) = payments(&server).await;
    let report = p
        .poll(&[
            "cus_A1".into(),
            "cus_B2".into(),
            "cus_GONE".into(),
            "cus_../x".into(),
        ])
        .await;
    assert_eq!((report.card, report.none), (1, 2));
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, "cus_../x");
    assert_eq!(
        sink.calls(),
        vec![
            ("cus_A1".to_owned(), PaymentState::Card),
            ("cus_B2".to_owned(), PaymentState::None),
            ("cus_GONE".to_owned(), PaymentState::None),
        ]
    );
}

#[tokio::test]
async fn ledger_is_written_before_the_request() {
    // A ledger whose writes are checked against what the server has seen:
    // the planned send must be durable before Stripe gets the request.
    struct Checking {
        inner: MemLedger,
        server: Arc<MockServer>,
    }
    impl std::fmt::Debug for Checking {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("Checking")
        }
    }
    #[async_trait]
    impl ExportLedger for Checking {
        async fn get(
            &self,
            k: &loams_meter_export::LedgerKey,
        ) -> Result<Option<loams_meter_export::LedgerEntry>, ExportError> {
            self.inner.get(k).await
        }
        async fn put(
            &self,
            k: &loams_meter_export::LedgerKey,
            e: &loams_meter_export::LedgerEntry,
        ) -> Result<(), ExportError> {
            let seen = self
                .server
                .received_requests()
                .await
                .expect("recording")
                .len();
            assert_eq!(
                seen,
                usize::from(e.sent),
                "planned before the send, marked sent after it"
            );
            self.inner.put(k, e).await
        }
    }
    let server = Arc::new(MockServer::start().await);
    accept(&server).await;
    let ledger = Arc::new(Checking {
        inner: MemLedger::new(),
        server: server.clone(),
    });
    let ex = StripeExporter::new(config(&server), ledger).expect("exporter");
    ex.export_period(&period(0, vec![hour(0, 1, 1)]))
        .await
        .expect("export");
}
