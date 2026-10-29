//! Threshold events and the notifier (ME1 Task 14): deterministic ids, so
//! an event emitted again is not delivered again; signed webhooks; retries.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use loams_authz::{BuiltinAuthorizer, Subject, Tuple};
use loams_meter::audit::MemAudit;
use loams_meter::authz::{Caller, MeterAuthz};
use loams_meter::customers::{Clock, Customers, NewCustomer};
use loams_meter::notify::events::{DEFAULT_THRESHOLDS, THRESHOLD};
use loams_meter::notify::notifier::{MemNotifications, NotificationStore};
use loams_meter::notify::webhook::{Outcome, Transport, verify};
use loams_meter::notify::{DeliveryStatus, Notifier, ThresholdTracker};
use loams_meter::owner::CounterKey;
use loams_meter::store::MemStore;
use loams_meter_core::{Amount, FeatureKey, Period, SubjectKey, UtcNanos};

/// One recorded request: URL, headers, body.
type Sent = (String, Vec<(String, String)>, Vec<u8>);

/// Records requests; answers the queued outcomes, then 204.
#[derive(Debug, Default)]
struct Recorder {
    sent: Mutex<Vec<Sent>>,
    answers: Mutex<Vec<Outcome>>,
}

#[async_trait]
impl Transport for Recorder {
    async fn post(&self, url: &str, headers: &[(&str, String)], body: &[u8]) -> Outcome {
        self.sent.lock().expect("lock").push((
            url.to_string(),
            headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
            body.to_vec(),
        ));
        let mut a = self.answers.lock().expect("lock");
        if a.is_empty() {
            Outcome::Delivered(204)
        } else {
            a.remove(0)
        }
    }
}

const T0: i64 = 1_700_000_000_000_000_000;

struct World {
    notifier: Notifier,
    transport: Arc<Recorder>,
    notes: Arc<MemNotifications>,
    customer_id: String,
    now: Arc<Mutex<i64>>,
}

async fn world() -> World {
    let fga = BuiltinAuthorizer::new();
    fga.write(&[
        Tuple::new("platform:default", "platform", "org:acme"),
        Tuple::new("user:mia", "meter_admin", "platform:default"),
        Tuple::new("user:bea", "billing_admin", "org:acme"),
    ])
    .expect("tuples");
    let authz = MeterAuthz::new(Arc::new(fga), "meter-export");
    let store = Arc::new(MemStore::new());
    let audit = Arc::new(MemAudit::new());
    let now = Arc::new(Mutex::new(T0));
    let n = now.clone();
    let clock: Clock = Arc::new(move || UtcNanos(*n.lock().expect("lock")));
    let customers = Customers::new(store.clone(), authz.clone(), audit.clone(), clock.clone());
    let c = customers
        .create(
            &Caller::Principal(Subject::user("mia")),
            NewCustomer {
                org: "acme".into(),
                subjects: vec!["org_acme".into()],
                ..NewCustomer::default()
            },
        )
        .await
        .expect("customer");
    let transport = Arc::new(Recorder::default());
    let notes = Arc::new(MemNotifications::new());
    let notifier = Notifier::new(
        store,
        notes.clone(),
        authz,
        audit,
        transport.clone(),
        None,
        None,
        Arc::new(|| [9; 32]),
        clock,
    );
    World {
        notifier,
        transport,
        notes,
        customer_id: c.id,
        now,
    }
}

fn key() -> CounterKey {
    CounterKey {
        subject: SubjectKey::new("org_acme").expect("subject"),
        feature: FeatureKey::new("house_cpu").expect("feature"),
        period_start: UtcNanos(0),
    }
}

const PERIOD: Period = Period {
    start: UtcNanos(0),
    end: UtcNanos(30 * UtcNanos::DAY),
};

#[tokio::test]
async fn threshold_event_reemission_is_duplicate() {
    let w = world().await;
    let bea = Caller::Principal(Subject::user("bea"));
    let (_, secret) = w
        .notifier
        .create_webhook(
            &bea,
            &w.customer_id,
            "https://hooks.example.com/meter",
            vec![],
        )
        .await
        .expect("webhook");

    // The owner sees usage cross 80% once: one event, delivered once.
    let mut tracker = ThresholdTracker::new("loams://meter/i1", DEFAULT_THRESHOLDS);
    let events = tracker.observe(
        &key(),
        PERIOD,
        Amount::from_units(85),
        Amount::from_units(100),
        UtcNanos(T0),
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].r#type, THRESHOLD);
    assert!(events[0].id.ends_with("/80"), "{}", events[0].id);
    assert!(
        tracker
            .observe(
                &key(),
                PERIOD,
                Amount::from_units(90),
                Amount::from_units(100),
                UtcNanos(T0)
            )
            .is_empty(),
        "a running owner emits each threshold once"
    );
    let h = w.notifier.handle(&events[0]).await.expect("handle");
    assert_eq!(h.delivered, 1);

    // A restarted owner (a fresh tracker) emits it again: same id, so it is
    // a duplicate and nothing is sent.
    let mut restarted = ThresholdTracker::new("loams://meter/i2", DEFAULT_THRESHOLDS);
    let again = restarted.observe(
        &key(),
        PERIOD,
        Amount::from_units(90),
        Amount::from_units(100),
        UtcNanos(T0 + 1),
    );
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id, events[0].id);
    let h = w.notifier.handle(&again[0]).await.expect("handle again");
    assert_eq!(h.delivered, 0);
    assert_eq!(h.duplicates, 1);
    assert_eq!(w.transport.sent.lock().expect("lock").len(), 1);

    // The one delivery is a signed structured CloudEvent.
    let (url, headers, body) = w.transport.sent.lock().expect("lock")[0].clone();
    assert_eq!(url, "https://hooks.example.com/meter");
    let get = |k: &str| {
        headers
            .iter()
            .find(|(h, _)| h == k)
            .map(|(_, v)| v.clone())
            .expect(k)
    };
    assert_eq!(get("webhook-id"), events[0].id);
    let ts: i64 = get("webhook-timestamp").parse().expect("timestamp");
    assert!(verify(
        &secret,
        &events[0].id,
        ts,
        &body,
        &get("webhook-signature"),
        ts
    ));
    let ce: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(ce["type"], THRESHOLD);
    assert_eq!(ce["data"]["threshold"], 80);

    // Crossing 100% is a new event.
    let full = tracker.observe(
        &key(),
        PERIOD,
        Amount::from_units(100),
        Amount::from_units(100),
        UtcNanos(T0),
    );
    assert_eq!(full.len(), 1);
    assert!(full[0].id.ends_with("/100"));
}

#[tokio::test]
async fn failed_deliveries_are_retried_on_schedule() {
    let w = world().await;
    let bea = Caller::Principal(Subject::user("bea"));
    w.notifier
        .create_webhook(
            &bea,
            &w.customer_id,
            "https://hooks.example.com/meter",
            vec![],
        )
        .await
        .expect("webhook");
    w.transport
        .answers
        .lock()
        .expect("lock")
        .push(Outcome::Status(503));
    let mut tracker = ThresholdTracker::new("loams://meter/i1", DEFAULT_THRESHOLDS);
    let e = tracker.observe(
        &key(),
        PERIOD,
        Amount::from_units(100),
        Amount::from_units(100),
        UtcNanos(T0),
    );
    // 80% and 100% at once.
    assert_eq!(e.len(), 2);
    let h = w.notifier.handle(&e[0]).await.expect("handle");
    assert_eq!(h.retrying, 1);
    // Not due yet.
    assert_eq!(w.notifier.retry_due().await.expect("retry"), 0);
    *w.now.lock().expect("lock") += 6 * UtcNanos::SECOND;
    assert_eq!(w.notifier.retry_due().await.expect("retry"), 1);
    let d = w
        .notes
        .delivery(
            &w.notes.endpoints_of(&w.customer_id).await.expect("eps")[0].id,
            &e[0].id,
        )
        .await
        .expect("read")
        .expect("delivery");
    assert_eq!(d.status, DeliveryStatus::Delivered);
    assert_eq!(d.attempt, 2);
}

#[tokio::test]
async fn webhooks_need_a_billing_admin_and_https() {
    let w = world().await;
    let mia = Caller::Principal(Subject::user("mia"));
    let err = w
        .notifier
        .create_webhook(&mia, &w.customer_id, "https://x.example.com", vec![])
        .await
        .expect_err("meter admin is not billing admin");
    assert_eq!(err.reason(), "billing_admin_required");
    let bea = Caller::Principal(Subject::user("bea"));
    assert!(
        w.notifier
            .create_webhook(&bea, &w.customer_id, "http://x.example.com", vec![])
            .await
            .is_err()
    );
    assert!(
        w.notifier
            .create_webhook(
                &bea,
                &w.customer_id,
                "https://x.example.com",
                vec!["com.other.v1".into()]
            )
            .await
            .is_err()
    );
    let agent = Caller::Principal(Subject::agent("a1"));
    assert_eq!(
        w.notifier
            .create_webhook(&agent, &w.customer_id, "https://x.example.com", vec![])
            .await
            .expect_err("agent")
            .reason(),
        "agent_not_allowed"
    );
}

#[tokio::test]
async fn webhook_secrets_are_sealed_at_rest_and_still_sign() {
    let mut w = world().await;
    let sealer = Arc::new(loams_meter::notify::sealing::AgeSealer::new(
        age::x25519::Identity::generate(),
    ));
    w.notifier = w.notifier.clone().with_sealer(sealer);
    let bea = Caller::Principal(Subject::user("bea"));
    let (ep, secret) = w
        .notifier
        .create_webhook(&bea, &w.customer_id, "https://hooks.example.com/m", vec![])
        .await
        .expect("webhook");
    // The record holds no plaintext.
    let stored = w.notes.endpoint(&ep.id).await.expect("read").expect("stored");
    assert!(stored.secret.starts_with("age:"), "{}", stored.secret);
    assert!(!stored.secret.contains(&secret));

    let mut tracker = ThresholdTracker::new("loams://meter/i1", DEFAULT_THRESHOLDS);
    let events = tracker.observe(
        &key(),
        PERIOD,
        Amount::from_units(85),
        Amount::from_units(100),
        UtcNanos(T0),
    );
    assert_eq!(w.notifier.handle(&events[0]).await.expect("handle").delivered, 1);
    let (_, headers, body) = w.transport.sent.lock().expect("lock")[0].clone();
    let get = |k: &str| {
        headers
            .iter()
            .find(|(h, _)| h == k)
            .map(|(_, v)| v.clone())
            .expect(k)
    };
    let ts: i64 = get("webhook-timestamp").parse().expect("timestamp");
    assert!(verify(&secret, &events[0].id, ts, &body, &get("webhook-signature"), ts));

    // A rotation seals the new secret too, and keeps the old one sealed.
    let (rotated, _) = w.notifier.rotate_secret(&bea, &ep.id).await.expect("rotate");
    assert!(rotated.secret.starts_with("age:"));
    assert!(rotated.previous.expect("previous").0.starts_with("age:"));
}
