//! The notifier's email and console channels (D2402): once per event,
//! retried on the webhook schedule, listed for the console; and the Resend
//! mailer against a local stand-in for Resend's API.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use loams_authz::{BuiltinAuthorizer, Subject, Tuple};
use loams_meter::audit::MemAudit;
use loams_meter::authz::{Caller, MeterAuthz};
use loams_meter::customers::{Clock, Customers, NewCustomer};
use loams_meter::notify::channels::{
    DocConsole, MailRecipients, MemConsole, ResendConfig, ResendMailer, RulesOnly, idempotency_key,
};
use loams_meter::notify::events::DEFAULT_THRESHOLDS;
use loams_meter::notify::notifier::{
    CONSOLE_CHANNEL, Console, EMAIL_CHANNEL, Mailer, MemNotifications, NotificationStore,
};
use loams_meter::notify::webhook::{Outcome, Transport};
use loams_meter::notify::{
    Channel, ChannelOutcome, DeliveryStatus, Mail, MeterEvent, Notifier, ThresholdTracker,
};
use loams_meter::owner::CounterKey;
use loams_meter::store::{Customer, MemDoc, MemStore};
use loams_meter_core::{Amount, FeatureKey, Period, SubjectKey, UtcNanos};

#[derive(Debug, Default)]
struct NoWebhooks;

#[async_trait]
impl Transport for NoWebhooks {
    async fn post(&self, _: &str, _: &[(&str, String)], _: &[u8]) -> Outcome {
        Outcome::Delivered(204)
    }
}

/// Records mails; answers the queued outcomes, then `Sent`.
#[derive(Debug, Default)]
struct Outbox {
    sent: Mutex<Vec<Mail>>,
    answers: Mutex<Vec<ChannelOutcome>>,
}

#[async_trait]
impl Mailer for Outbox {
    async fn send(&self, _: &Customer, mail: &Mail) -> ChannelOutcome {
        self.sent.lock().expect("lock").push(mail.clone());
        let mut a = self.answers.lock().expect("lock");
        if a.is_empty() {
            ChannelOutcome::Sent("ok".into())
        } else {
            a.remove(0)
        }
    }
}

/// A console that fails its first `fail` notifications.
#[derive(Debug, Default)]
struct FlakyConsole {
    inner: MemConsole,
    fail: Mutex<u32>,
}

#[async_trait]
impl Console for FlakyConsole {
    async fn notify(
        &self,
        n: loams_meter::notify::ConsoleNotification,
    ) -> loams_meter::error::Result<()> {
        {
            let mut f = self.fail.lock().expect("lock");
            if *f > 0 {
                *f -= 1;
                return Err(loams_meter::error::MeterError::Unavailable(
                    "store down".into(),
                ));
            }
        }
        self.inner.notify(n).await
    }

    async fn list(
        &self,
        customer_id: &str,
        limit: usize,
    ) -> loams_meter::error::Result<Vec<loams_meter::notify::ConsoleNotification>> {
        self.inner.list(customer_id, limit).await
    }
}

const T0: i64 = 1_700_000_000_000_000_000;

struct World {
    notifier: Notifier,
    outbox: Arc<Outbox>,
    console: Arc<FlakyConsole>,
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
    let outbox = Arc::new(Outbox::default());
    let console = Arc::new(FlakyConsole::default());
    let notes = Arc::new(MemNotifications::new());
    let notifier = Notifier::new(
        store,
        notes.clone(),
        authz,
        audit,
        Arc::new(NoWebhooks),
        Some(outbox.clone()),
        Some(console.clone()),
        Arc::new(|| [9; 32]),
        clock,
    );
    World {
        notifier,
        outbox,
        console,
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

/// 80 % and 100 % of the allowance.
fn thresholds() -> Vec<MeterEvent> {
    let mut tracker = ThresholdTracker::new("loams://meter/i1", DEFAULT_THRESHOLDS);
    tracker.observe(
        &key(),
        PERIOD,
        Amount::from_units(100),
        Amount::from_units(100),
        UtcNanos(T0),
    )
}

fn bea() -> Caller {
    Caller::Principal(Subject::user("bea"))
}

#[tokio::test]
async fn email_goes_once_per_event_to_the_rules_addresses() {
    let w = world().await;
    w.notifier
        .create_rule(
            &bea(),
            &w.customer_id,
            Some("house_cpu"),
            vec![80],
            false,
            vec![Channel::Email],
            vec!["ops@acme.example".into()],
        )
        .await
        .expect("rule");
    let events = thresholds();
    let h = w.notifier.handle(&events[0]).await.expect("handle 80");
    assert_eq!((h.emails, h.console), (1, 1));
    // 100 % matches no email rule: console only.
    let h = w.notifier.handle(&events[1]).await.expect("handle 100");
    assert_eq!((h.emails, h.console), (0, 1));
    // The 80 % event emitted again: neither channel sends again.
    let h = w.notifier.handle(&events[0]).await.expect("again");
    assert_eq!((h.emails, h.console, h.duplicates), (0, 0, 2));

    let sent = w.outbox.sent.lock().expect("lock").clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, ["ops@acme.example"]);
    assert_eq!(
        sent[0].subject,
        "house_cpu is at 80% of this period's allowance"
    );
    assert!(
        sent[0].text.contains("Used: 100 of 100"),
        "{}",
        sent[0].text
    );
    assert_eq!(sent[0].idempotency_key, format!("{}/email", events[0].id));

    // The console lists both, newest first; 100 % is a banner.
    let listed = w
        .notifier
        .list_console(&bea(), &w.customer_id, 10)
        .await
        .expect("list");
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|n| n.id == events[1].id && n.banner));
    assert!(listed.iter().any(|n| n.id == events[0].id && !n.banner));
    // Both channels are in the delivery log.
    let log = w
        .notes
        .deliveries_of(&w.customer_id, 10)
        .await
        .expect("log");
    assert_eq!(
        log.iter()
            .filter(|d| d.endpoint_id == EMAIL_CHANNEL)
            .count(),
        1
    );
    assert_eq!(
        log.iter()
            .filter(|d| d.endpoint_id == CONSOLE_CHANNEL)
            .count(),
        2
    );
}

#[tokio::test]
async fn failed_email_and_console_are_retried_on_schedule() {
    let w = world().await;
    w.notifier
        .create_rule(
            &bea(),
            &w.customer_id,
            None,
            vec![80],
            false,
            vec![Channel::Email, Channel::Console],
            vec![],
        )
        .await
        .expect("rule");
    w.outbox
        .answers
        .lock()
        .expect("lock")
        .push(ChannelOutcome::Retry("resend 503".into()));
    *w.console.fail.lock().expect("lock") = 1;
    let e = &thresholds()[0];
    let h = w.notifier.handle(e).await.expect("handle");
    assert_eq!((h.emails, h.console, h.retrying), (0, 0, 2));
    // Not due for 5 s.
    assert_eq!(w.notifier.retry_due().await.expect("retry"), 0);
    *w.now.lock().expect("lock") += 6 * UtcNanos::SECOND;
    assert_eq!(w.notifier.retry_due().await.expect("retry"), 2);
    for channel in [EMAIL_CHANNEL, CONSOLE_CHANNEL] {
        let d = w
            .notes
            .delivery(channel, &e.id)
            .await
            .expect("read")
            .expect("delivery");
        assert_eq!(d.status, DeliveryStatus::Delivered, "{channel}");
        assert_eq!(d.attempt, 2, "{channel}");
    }
    let sent = w.outbox.sent.lock().expect("lock").clone();
    assert_eq!(sent.len(), 2);
    // No addresses on the rule: the mailer's default recipients.
    assert!(sent[1].to.is_empty());
    assert_eq!(sent[0].idempotency_key, sent[1].idempotency_key);
    assert_eq!(
        w.console
            .list(&w.customer_id, 10)
            .await
            .expect("list")
            .len(),
        1
    );
}

#[tokio::test]
async fn a_refused_email_is_not_retried() {
    let w = world().await;
    w.notifier
        .create_rule(
            &bea(),
            &w.customer_id,
            None,
            vec![80],
            false,
            vec![Channel::Email],
            vec![],
        )
        .await
        .expect("rule");
    w.outbox
        .answers
        .lock()
        .expect("lock")
        .push(ChannelOutcome::Failed("no recipients".into()));
    let e = &thresholds()[0];
    let h = w.notifier.handle(e).await.expect("handle");
    assert_eq!(h.failed, 1);
    *w.now.lock().expect("lock") += 3600 * UtcNanos::SECOND;
    assert_eq!(w.notifier.retry_due().await.expect("retry"), 0);
    let d = w
        .notes
        .delivery(EMAIL_CHANNEL, &e.id)
        .await
        .expect("read")
        .expect("delivery");
    assert_eq!(
        (d.status, d.last_outcome.as_str()),
        (DeliveryStatus::Failed, "no recipients")
    );
}

#[tokio::test]
async fn rule_addresses_are_checked() {
    let w = world().await;
    let rule = |channels: Vec<Channel>, emails: Vec<&'static str>| {
        let n = w.notifier.clone();
        let id = w.customer_id.clone();
        async move {
            n.create_rule(
                &bea(),
                &id,
                None,
                vec![80],
                false,
                channels,
                emails.into_iter().map(String::from).collect(),
            )
            .await
        }
    };
    assert!(
        rule(vec![Channel::Email], vec!["not an address"])
            .await
            .is_err()
    );
    assert!(
        rule(vec![Channel::Email], vec!["a@localhost"])
            .await
            .is_err()
    );
    assert!(
        rule(vec![Channel::Console], vec!["a@b.example"])
            .await
            .is_err()
    );
    assert!(
        rule(vec![Channel::Email], vec!["x@y.example"; 11])
            .await
            .is_err()
    );
    let ok = rule(vec![Channel::Email], vec!["a@b.example"])
        .await
        .expect("valid");
    assert_eq!(ok.emails, ["a@b.example"]);
    // A meter admin who is not the org's billing admin cannot list.
    assert_eq!(
        w.notifier
            .list_console(
                &Caller::Principal(Subject::user("nobody")),
                &w.customer_id,
                10
            )
            .await
            .expect_err("no billing view")
            .reason(),
        "billing_viewer_required"
    );
}

#[tokio::test]
async fn the_console_document_keeps_one_notification_per_event() {
    let console = DocConsole::new(Arc::new(MemDoc::default()));
    let n = |id: &str, at: i64| loams_meter::notify::ConsoleNotification {
        id: id.into(),
        customer_id: "c1".into(),
        event_type: "io.loams.dev.meter.threshold.v1".into(),
        feature: "house_cpu".into(),
        title: "t".into(),
        body: "b".into(),
        banner: false,
        at: UtcNanos(at),
    };
    console.notify(n("e1", 1)).await.expect("one");
    console.notify(n("e2", 2)).await.expect("two");
    console.notify(n("e1", 3)).await.expect("again");
    let listed = console.list("c1", 10).await.expect("list");
    assert_eq!(
        listed.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
        ["e2", "e1"]
    );
    assert!(console.list("c2", 10).await.expect("list").is_empty());
}

/// Who a request to the stand-in Resend was for, and with which headers.
type Seen = Arc<Mutex<Vec<(Option<String>, Option<String>, serde_json::Value)>>>;

/// A local stand-in for Resend's `POST /emails`: answers each queued
/// status, then 200 with an id.
async fn fake_resend(statuses: Vec<u16>) -> (String, Seen) {
    // The process's TLS provider, as the loams binary installs it.
    let _ = rustls::crypto::ring::default_provider().install_default();
    use axum::http::{HeaderMap, StatusCode};
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let queue = Arc::new(Mutex::new(statuses));
    let s = seen.clone();
    let app = axum::Router::new().route(
        "/emails",
        axum::routing::post(move |headers: HeaderMap, body: axum::body::Bytes| {
            let s = s.clone();
            let queue = queue.clone();
            async move {
                let get = |k: &str| {
                    headers
                        .get(k)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string)
                };
                s.lock().expect("lock").push((
                    get("authorization"),
                    get("idempotency-key"),
                    serde_json::from_slice(&body).expect("json"),
                ));
                let status = {
                    let mut q = queue.lock().expect("lock");
                    if q.is_empty() { 200 } else { q.remove(0) }
                };
                let code = StatusCode::from_u16(status).expect("status");
                if status == 200 {
                    (code, "{\"id\":\"em_1\"}".to_string())
                } else {
                    (code, "{\"message\":\"slow down\"}".to_string())
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (format!("http://{addr}"), seen)
}

#[derive(Debug)]
struct Admins;

#[async_trait]
impl MailRecipients for Admins {
    async fn default_to(&self, _: &Customer) -> loams_meter::error::Result<Vec<String>> {
        Ok(vec![
            "owner@acme.example".into(),
            "admin@acme.example".into(),
        ])
    }
}

fn customer() -> Customer {
    Customer {
        id: "c1".into(),
        org: "acme".into(),
        display_name: String::new(),
        subjects: vec![SubjectKey::new("org_acme").expect("subject")],
        payment_state: loams_meter::store::PaymentState::None,
        spend_controls: loams_meter::store::SpendControls::default(),
        stripe_customer_id: None,
        currency: loams_meter_core::Currency::EUR,
        plan: None,
        created_at: UtcNanos(0),
        updated_at: UtcNanos(0),
        version: 1,
    }
}

fn mail(to: &[&str]) -> Mail {
    Mail {
        to: to.iter().map(|s| s.to_string()).collect(),
        subject: "house_cpu is at 80% of this period's allowance".into(),
        text: "Used: 80 of 100".into(),
        idempotency_key: "org_acme/house_cpu/x/80/email".into(),
    }
}

#[tokio::test]
async fn resend_gets_the_mail_with_its_key_and_idempotency_key() {
    let (base, seen) = fake_resend(vec![429]).await;
    let mut cfg = ResendConfig::new("re_test_key", "Loams <billing@loams.example>");
    cfg.api_base = base;
    let mailer = ResendMailer::new(cfg, Arc::new(Admins)).expect("mailer");
    // 429 is worth retrying; the retry carries the same idempotency key.
    let first = mailer.send(&customer(), &mail(&[])).await;
    assert!(
        matches!(first, ChannelOutcome::Retry(ref m) if m.contains("429")),
        "{first:?}"
    );
    let second = mailer.send(&customer(), &mail(&[])).await;
    assert_eq!(second, ChannelOutcome::Sent("resend em_1".into()));
    let seen = seen.lock().expect("lock").clone();
    assert_eq!(seen.len(), 2);
    let (auth, key, body) = &seen[1];
    assert_eq!(auth.as_deref(), Some("Bearer re_test_key"));
    assert_eq!(key.as_deref(), Some("org_acme/house_cpu/x/80/email"));
    assert_eq!(seen[0].1, seen[1].1);
    assert_eq!(body["from"], "Loams <billing@loams.example>");
    // No rule addresses: the org's owners and admins, sorted.
    assert_eq!(
        body["to"],
        serde_json::json!(["admin@acme.example", "owner@acme.example"])
    );
    assert_eq!(body["text"], "Used: 80 of 100");
}

#[tokio::test]
async fn resend_refusals_and_missing_recipients_fail_for_good() {
    let (base, _) = fake_resend(vec![422]).await;
    let mut cfg = ResendConfig::new("re_test_key", "billing@loams.example");
    cfg.api_base = base;
    let mailer = ResendMailer::new(cfg.clone(), Arc::new(RulesOnly)).expect("mailer");
    assert!(matches!(
        mailer.send(&customer(), &mail(&["x@y.example"])).await,
        ChannelOutcome::Failed(_)
    ));
    assert_eq!(
        mailer.send(&customer(), &mail(&[])).await,
        ChannelOutcome::Failed("no recipients".into())
    );
    assert!(ResendMailer::new(ResendConfig::new(" ", "a@b.example"), Arc::new(RulesOnly)).is_err());
    // A key longer than Resend's 256 characters is hashed.
    let long = "k".repeat(300);
    assert!(idempotency_key(&long).starts_with("loams:h:"));
    assert_eq!(idempotency_key("short"), "short");
}
