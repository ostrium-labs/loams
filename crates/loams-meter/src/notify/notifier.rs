//! The notifier (design §63 §15, ME1 Task 14) and the domain of
//! `NotificationService` (§17.1): webhook endpoints, rules and deliveries.
//!
//! The notifier reads the meter's own events off `_system/_meter` (the
//! counter owner appended them) and delivers each to:
//! - the customer's webhook endpoints that subscribe to its type, signed
//!   ([`super::webhook`]), retried on [`super::webhook::RETRY_AFTER`]'s
//!   schedule; one delivery per (endpoint, event id), so an event emitted
//!   again is not delivered again;
//! - email through the deployment's mailer, when it has one, for the
//!   thresholds a rule asks for;
//! - the console (notifications and the shell banner).

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use loams_meter_core::{FeatureKey, SubjectKey, UtcNanos};

use super::events::{ACCESS_CHANGED, MeterEvent, THRESHOLD};
use super::sealing::{SecretSealer, Unsealed};
use super::webhook::{self, Outcome, RETRY_AFTER, Transport};
use crate::audit::{AuditEvent, AuditSink};
use crate::authz::{Caller, MeterAuthz};
use crate::customers::{Clock, new_id};
use crate::error::{MeterError, Result};
use crate::store::{Customer, MeterStore};

/// How long a rotated-out secret still signs deliveries.
pub const ROTATION_OVERLAP: Duration = Duration::from_secs(24 * 3600);

/// The most endpoints one customer may have.
pub const MAX_ENDPOINTS: usize = 20;

/// A webhook endpoint of a customer.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Endpoint {
    pub id: String,
    pub customer_id: String,
    pub url: String,
    /// The current secret, sealed ([`super::sealing`]); a rotated-out one
    /// in `previous` until its time.
    pub secret: String,
    pub previous: Option<(String, UtcNanos)>,
    /// The event types it receives; empty: every `io.loams.dev.meter.*`.
    pub types: Vec<String>,
    pub created_at: UtcNanos,
}

impl Endpoint {
    fn wants(&self, ty: &str) -> bool {
        self.types.is_empty() || self.types.iter().any(|t| t == ty)
    }

    fn secrets(&self, now: UtcNanos, sealer: &dyn SecretSealer) -> Result<Vec<String>> {
        let mut s = vec![sealer.open(&self.secret)?];
        if let Some((old, until)) = &self.previous
            && now < *until
        {
            s.push(sealer.open(old)?);
        }
        Ok(s)
    }
}

/// Where a rule sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Channel {
    Email,
    Console,
}

/// A notification rule: which thresholds of which feature reach which
/// channels (webhooks get every subscribed event).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rule {
    pub id: String,
    pub customer_id: String,
    /// `None`: every feature.
    pub feature: Option<FeatureKey>,
    pub thresholds: Vec<u32>,
    /// Also the spend-cap thresholds.
    pub spend_cap: bool,
    pub channels: Vec<Channel>,
}

impl Rule {
    fn matches(&self, event: &MeterEvent) -> bool {
        let feature_ok = self
            .feature
            .as_ref()
            .is_none_or(|f| event.data.get("feature").and_then(|v| v.as_str()) == Some(f.as_str()));
        if !feature_ok {
            return false;
        }
        if event.r#type == ACCESS_CHANGED {
            return true;
        }
        if event.r#type != THRESHOLD {
            return false;
        }
        let spend = event.data.get("basis").and_then(|v| v.as_str()) == Some("spend_cap");
        if spend && !self.spend_cap {
            return false;
        }
        let t = event
            .data
            .get("threshold")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        self.thresholds.iter().any(|x| u64::from(*x) == t)
    }
}

/// The state of one delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeliveryStatus {
    Delivered,
    /// Will be tried again at `next_attempt`.
    Retrying,
    /// Gave up (a non-retryable answer, or the schedule ran out).
    Failed,
}

/// One event to one endpoint.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Delivery {
    pub endpoint_id: String,
    pub customer_id: String,
    pub event_id: String,
    pub event_type: String,
    pub body: Vec<u8>,
    pub attempt: u32,
    pub at: UtcNanos,
    pub status: DeliveryStatus,
    pub last_outcome: String,
    pub next_attempt: Option<UtcNanos>,
}

/// Endpoints, rules and deliveries. The role keeps them in the
/// `ControlStore`.
#[async_trait]
pub trait NotificationStore: Send + Sync + fmt::Debug {
    async fn put_endpoint(&self, e: Endpoint) -> Result<()>;
    async fn endpoint(&self, id: &str) -> Result<Option<Endpoint>>;
    async fn endpoints_of(&self, customer_id: &str) -> Result<Vec<Endpoint>>;
    async fn put_rule(&self, r: Rule) -> Result<()>;
    async fn rules_of(&self, customer_id: &str) -> Result<Vec<Rule>>;
    async fn delivery(&self, endpoint_id: &str, event_id: &str) -> Result<Option<Delivery>>;
    async fn put_delivery(&self, d: Delivery) -> Result<()>;
    async fn due(&self, now: UtcNanos) -> Result<Vec<Delivery>>;
    /// The newest first, at most `limit`.
    async fn deliveries_of(&self, customer_id: &str, limit: usize) -> Result<Vec<Delivery>>;
}

/// Email through the deployment's mailer (none on the desktop).
#[async_trait]
pub trait Mailer: Send + Sync + fmt::Debug {
    async fn send(&self, customer: &Customer, subject: &str, body: &str) -> Result<()>;
}

/// Console notifications and the shell banner.
#[async_trait]
pub trait Console: Send + Sync + fmt::Debug {
    async fn notify(&self, customer: &Customer, event: &MeterEvent) -> Result<()>;
}

/// [`NotificationStore`] in memory.
#[derive(Debug, Default)]
pub struct MemNotifications {
    inner: Mutex<MemInner>,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct MemInner {
    endpoints: Vec<Endpoint>,
    rules: Vec<Rule>,
    deliveries: Vec<Delivery>,
}

impl MemNotifications {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl MemInner {
    fn put_endpoint(&mut self, e: Endpoint) -> Result<()> {
        let s = self;
        s.endpoints.retain(|x| x.id != e.id);
        s.endpoints.push(e);
        Ok(())
    }
    fn endpoint(&mut self, id: &str) -> Result<Option<Endpoint>> {
        Ok(self.endpoints.iter().find(|e| e.id == id).cloned())
    }
    fn endpoints_of(&mut self, customer_id: &str) -> Result<Vec<Endpoint>> {
        Ok(self
            .endpoints
            .iter()
            .filter(|e| e.customer_id == customer_id)
            .cloned()
            .collect())
    }
    fn put_rule(&mut self, r: Rule) -> Result<()> {
        let s = self;
        s.rules.retain(|x| x.id != r.id);
        s.rules.push(r);
        Ok(())
    }
    fn rules_of(&mut self, customer_id: &str) -> Result<Vec<Rule>> {
        Ok(self
            .rules
            .iter()
            .filter(|r| r.customer_id == customer_id)
            .cloned()
            .collect())
    }
    fn delivery(&mut self, endpoint_id: &str, event_id: &str) -> Result<Option<Delivery>> {
        Ok(self
            .deliveries
            .iter()
            .find(|d| d.endpoint_id == endpoint_id && d.event_id == event_id)
            .cloned())
    }
    fn put_delivery(&mut self, d: Delivery) -> Result<()> {
        let s = self;
        s.deliveries
            .retain(|x| !(x.endpoint_id == d.endpoint_id && x.event_id == d.event_id));
        s.deliveries.push(d);
        Ok(())
    }
    fn due(&mut self, now: UtcNanos) -> Result<Vec<Delivery>> {
        Ok(self
            .deliveries
            .iter()
            .filter(|d| {
                d.status == DeliveryStatus::Retrying && d.next_attempt.is_some_and(|t| t <= now)
            })
            .cloned()
            .collect())
    }
    fn deliveries_of(&mut self, customer_id: &str, limit: usize) -> Result<Vec<Delivery>> {
        let mut all: Vec<Delivery> = self
            .deliveries
            .iter()
            .filter(|d| d.customer_id == customer_id)
            .cloned()
            .collect();
        all.sort_by_key(|d| std::cmp::Reverse(d.at));
        all.truncate(limit);
        Ok(all)
    }
}

#[async_trait]
impl NotificationStore for MemNotifications {
    async fn put_endpoint(&self, e: Endpoint) -> Result<()> {
        let s = &mut *self.lock();
        s.put_endpoint(e)
    }
    async fn endpoint(&self, id: &str) -> Result<Option<Endpoint>> {
        let s = &mut *self.lock();
        s.endpoint(id)
    }
    async fn endpoints_of(&self, customer_id: &str) -> Result<Vec<Endpoint>> {
        let s = &mut *self.lock();
        s.endpoints_of(customer_id)
    }
    async fn put_rule(&self, r: Rule) -> Result<()> {
        let s = &mut *self.lock();
        s.put_rule(r)
    }
    async fn rules_of(&self, customer_id: &str) -> Result<Vec<Rule>> {
        let s = &mut *self.lock();
        s.rules_of(customer_id)
    }
    async fn delivery(&self, endpoint_id: &str, event_id: &str) -> Result<Option<Delivery>> {
        let s = &mut *self.lock();
        s.delivery(endpoint_id, event_id)
    }
    async fn put_delivery(&self, d: Delivery) -> Result<()> {
        let s = &mut *self.lock();
        s.put_delivery(d)
    }
    async fn due(&self, now: UtcNanos) -> Result<Vec<Delivery>> {
        let s = &mut *self.lock();
        s.due(now)
    }
    async fn deliveries_of(&self, customer_id: &str, limit: usize) -> Result<Vec<Delivery>> {
        let s = &mut *self.lock();
        s.deliveries_of(customer_id, limit)
    }
}

/// The most deliveries kept per customer; older ones are dropped.
pub const DELIVERIES_KEPT: usize = 1000;

/// [`NotificationStore`] on a [`crate::store::VersionedDoc`] (endpoints,
/// rules and the newest [`DELIVERIES_KEPT`] deliveries of each customer),
/// written the same way as [`crate::store::DocStore`].
pub struct DocNotifications {
    doc: Arc<dyn crate::store::VersionedDoc>,
    writes: tokio::sync::Mutex<()>,
}

impl fmt::Debug for DocNotifications {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocNotifications").finish_non_exhaustive()
    }
}

impl DocNotifications {
    pub fn new(doc: Arc<dyn crate::store::VersionedDoc>) -> Self {
        DocNotifications {
            doc,
            writes: tokio::sync::Mutex::new(()),
        }
    }

    async fn current(&self) -> Result<(Option<u64>, MemInner)> {
        match self.doc.load().await? {
            None => Ok((None, MemInner::default())),
            Some((v, bytes)) => Ok((
                Some(v),
                serde_json::from_slice(&bytes).map_err(|e| {
                    MeterError::Unavailable(format!("the notification records do not parse: {e}"))
                })?,
            )),
        }
    }

    async fn read<R>(&self, f: impl FnOnce(&mut MemInner) -> Result<R>) -> Result<R> {
        let (_, mut inner) = self.current().await?;
        f(&mut inner)
    }

    async fn write<R>(&self, mut f: impl FnMut(&mut MemInner) -> Result<R>) -> Result<R> {
        let _one = self.writes.lock().await;
        for _ in 0..8 {
            let (version, mut inner) = self.current().await?;
            let out = f(&mut inner)?;
            inner.trim();
            let bytes = serde_json::to_vec(&inner).map_err(|e| {
                MeterError::Unavailable(format!("the notification records do not serialise: {e}"))
            })?;
            if self.doc.save(version, bytes).await?.is_some() {
                return Ok(out);
            }
        }
        Err(MeterError::Unavailable(
            "the notification records changed concurrently too often; retry".into(),
        ))
    }
}

impl MemInner {
    /// Keeps the newest deliveries of each customer, and every one still
    /// retrying.
    fn trim(&mut self) {
        let mut per: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        self.deliveries.sort_by_key(|d| std::cmp::Reverse(d.at));
        self.deliveries.retain(|d| {
            let n = per.entry(d.customer_id.clone()).or_insert(0);
            *n += 1;
            *n <= DELIVERIES_KEPT || d.status == DeliveryStatus::Retrying
        });
    }
}

#[async_trait]
impl NotificationStore for DocNotifications {
    async fn put_endpoint(&self, e: Endpoint) -> Result<()> {
        self.write(|s| s.put_endpoint(e.clone())).await
    }
    async fn endpoint(&self, id: &str) -> Result<Option<Endpoint>> {
        self.read(|s| s.endpoint(id)).await
    }
    async fn endpoints_of(&self, customer_id: &str) -> Result<Vec<Endpoint>> {
        self.read(|s| s.endpoints_of(customer_id)).await
    }
    async fn put_rule(&self, r: Rule) -> Result<()> {
        self.write(|s| s.put_rule(r.clone())).await
    }
    async fn rules_of(&self, customer_id: &str) -> Result<Vec<Rule>> {
        self.read(|s| s.rules_of(customer_id)).await
    }
    async fn delivery(&self, endpoint_id: &str, event_id: &str) -> Result<Option<Delivery>> {
        self.read(|s| s.delivery(endpoint_id, event_id)).await
    }
    async fn put_delivery(&self, d: Delivery) -> Result<()> {
        self.write(|s| s.put_delivery(d.clone())).await
    }
    async fn due(&self, now: UtcNanos) -> Result<Vec<Delivery>> {
        self.read(|s| s.due(now)).await
    }
    async fn deliveries_of(&self, customer_id: &str, limit: usize) -> Result<Vec<Delivery>> {
        self.read(|s| s.deliveries_of(customer_id, limit)).await
    }
}

/// The notifier and the `NotificationService` domain.
#[derive(Clone)]
pub struct Notifier {
    store: Arc<dyn MeterStore>,
    notes: Arc<dyn NotificationStore>,
    authz: MeterAuthz,
    audit: Arc<dyn AuditSink>,
    transport: Arc<dyn Transport>,
    mailer: Option<Arc<dyn Mailer>>,
    console: Option<Arc<dyn Console>>,
    /// 32 random bytes per call, for new secrets.
    random: Arc<dyn Fn() -> [u8; 32] + Send + Sync>,
    clock: Clock,
    /// How webhook secrets are kept at rest.
    sealer: Arc<dyn SecretSealer>,
}

impl fmt::Debug for Notifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Notifier").finish_non_exhaustive()
    }
}

fn nanos(d: Duration) -> i64 {
    i64::try_from(d.as_nanos()).unwrap_or(i64::MAX)
}

fn outcome_text(o: &Outcome) -> String {
    match o {
        Outcome::Delivered(s) | Outcome::Status(s) => s.to_string(),
        Outcome::Failed(e) => e.clone(),
    }
}

/// What one `handle` did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handled {
    pub delivered: usize,
    pub retrying: usize,
    pub failed: usize,
    /// Endpoints that already had this event (re-emission).
    pub duplicates: usize,
    pub emails: usize,
    pub console: usize,
}

impl Notifier {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<dyn MeterStore>,
        notes: Arc<dyn NotificationStore>,
        authz: MeterAuthz,
        audit: Arc<dyn AuditSink>,
        transport: Arc<dyn Transport>,
        mailer: Option<Arc<dyn Mailer>>,
        console: Option<Arc<dyn Console>>,
        random: Arc<dyn Fn() -> [u8; 32] + Send + Sync>,
        clock: Clock,
    ) -> Self {
        Notifier {
            store,
            notes,
            authz,
            audit,
            transport,
            mailer,
            console,
            random,
            clock,
            sealer: Arc::new(Unsealed),
        }
    }

    /// Seals webhook secrets at rest with `sealer` (the role always does;
    /// [`Unsealed`] is the default for tests).
    pub fn with_sealer(mut self, sealer: Arc<dyn SecretSealer>) -> Self {
        self.sealer = sealer;
        self
    }

    async fn attempt(&self, ep: &Endpoint, mut d: Delivery) -> Result<Delivery> {
        let now = (self.clock)();
        let ts = now.0.div_euclid(1_000_000_000);
        let headers = webhook::headers(&ep.secrets(now, &*self.sealer)?, &d.event_id, ts, &d.body)?;
        let outcome = self.transport.post(&ep.url, &headers, &d.body).await;
        d.attempt += 1;
        d.at = now;
        d.last_outcome = outcome_text(&outcome);
        if outcome.is_success() {
            d.status = DeliveryStatus::Delivered;
            d.next_attempt = None;
        } else if outcome.retryable() && (d.attempt as usize) <= RETRY_AFTER.len() {
            d.status = DeliveryStatus::Retrying;
            d.next_attempt = Some(now.add_nanos(nanos(RETRY_AFTER[d.attempt as usize - 1])));
        } else {
            d.status = DeliveryStatus::Failed;
            d.next_attempt = None;
        }
        self.notes.put_delivery(d.clone()).await?;
        Ok(d)
    }

    /// Delivers one meter event to everything that wants it.
    pub async fn handle(&self, event: &MeterEvent) -> Result<Handled> {
        let mut h = Handled::default();
        let subject = SubjectKey::new(event.subject.clone())
            .map_err(|e| MeterError::invalid("subject", e.to_string()))?;
        let Some(customer) = self.store.customer_by_subject(&subject).await? else {
            return Ok(h);
        };
        let body = serde_json::to_vec(&event.to_cloudevent_json())
            .map_err(|e| MeterError::Unavailable(format!("event body: {e}")))?;
        for ep in self.notes.endpoints_of(&customer.id).await? {
            if !ep.wants(&event.r#type) {
                continue;
            }
            if self.notes.delivery(&ep.id, &event.id).await?.is_some() {
                h.duplicates += 1;
                continue;
            }
            let d = Delivery {
                endpoint_id: ep.id.clone(),
                customer_id: customer.id.clone(),
                event_id: event.id.clone(),
                event_type: event.r#type.clone(),
                body: body.clone(),
                attempt: 0,
                at: (self.clock)(),
                status: DeliveryStatus::Retrying,
                last_outcome: String::new(),
                next_attempt: None,
            };
            match self.attempt(&ep, d).await?.status {
                DeliveryStatus::Delivered => h.delivered += 1,
                DeliveryStatus::Retrying => h.retrying += 1,
                DeliveryStatus::Failed => h.failed += 1,
            }
        }
        let rules = self.notes.rules_of(&customer.id).await?;
        let channels: Vec<Channel> = rules
            .iter()
            .filter(|r| r.matches(event))
            .flat_map(|r| r.channels.iter().copied())
            .collect();
        // The console always hears about thresholds and access changes (the
        // banner); email only where a rule asks for it.
        if let Some(console) = &self.console
            && (event.r#type == THRESHOLD || event.r#type == ACCESS_CHANGED)
        {
            console.notify(&customer, event).await?;
            h.console += 1;
        }
        if let Some(mailer) = &self.mailer
            && channels.contains(&Channel::Email)
        {
            let feature = event
                .data
                .get("feature")
                .and_then(|v| v.as_str())
                .unwrap_or("usage");
            let subject = match event.r#type.as_str() {
                THRESHOLD => format!(
                    "{feature} is at {}% of this period's allowance",
                    event
                        .data
                        .get("threshold")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                ),
                _ => format!("{feature}: access changed"),
            };
            mailer
                .send(
                    &customer,
                    &subject,
                    &serde_json::to_string_pretty(&event.data).unwrap_or_default(),
                )
                .await?;
            h.emails += 1;
        }
        Ok(h)
    }

    /// Retries every delivery that is due. Answers how many were tried.
    pub async fn retry_due(&self) -> Result<usize> {
        let now = (self.clock)();
        let due = self.notes.due(now).await?;
        let mut n = 0;
        for d in due {
            let Some(ep) = self.notes.endpoint(&d.endpoint_id).await? else {
                let mut gone = d;
                gone.status = DeliveryStatus::Failed;
                gone.next_attempt = None;
                gone.last_outcome = "endpoint deleted".into();
                self.notes.put_delivery(gone).await?;
                continue;
            };
            self.attempt(&ep, d).await?;
            n += 1;
        }
        Ok(n)
    }

    async fn customer_for_admin(&self, caller: &Caller, customer_id: &str) -> Result<Customer> {
        let c = self
            .store
            .customer(customer_id)
            .await?
            .ok_or_else(|| MeterError::not_found("customer", customer_id))?;
        self.authz.require_billing_admin(caller, &c.org).await?;
        Ok(c)
    }

    async fn audit(
        &self,
        caller: &Caller,
        action: &str,
        c: &Customer,
        object: String,
    ) -> Result<()> {
        self.audit
            .record(AuditEvent {
                action: action.into(),
                actor: caller.actor(),
                object,
                org: Some(c.org.clone()),
                reason: String::new(),
                details: serde_json::Value::Null,
                time: (self.clock)(),
            })
            .await
    }

    /// `CreateWebhook` (`billing_admin`): answers the endpoint and its
    /// secret, which is never shown again.
    pub async fn create_webhook(
        &self,
        caller: &Caller,
        customer_id: &str,
        url: &str,
        types: Vec<String>,
    ) -> Result<(Endpoint, String)> {
        let c = self.customer_for_admin(caller, customer_id).await?;
        webhook::check_url(url)?;
        if let Some(bad) = types.iter().find(|t| !t.starts_with("io.loams.dev.meter.")) {
            return Err(MeterError::invalid(
                "types",
                format!("{bad} is not a meter event type"),
            ));
        }
        if self.notes.endpoints_of(&c.id).await?.len() >= MAX_ENDPOINTS {
            return Err(MeterError::precondition(
                "too_many_webhooks",
                format!("a customer has at most {MAX_ENDPOINTS} webhook endpoints"),
            ));
        }
        let secret = webhook::new_secret((self.random)());
        let ep = Endpoint {
            id: new_id(),
            customer_id: c.id.clone(),
            url: url.to_string(),
            secret: self.sealer.seal(&secret)?,
            previous: None,
            types,
            created_at: (self.clock)(),
        };
        self.notes.put_endpoint(ep.clone()).await?;
        self.audit(
            caller,
            "meter.webhook.create",
            &c,
            format!("webhook:{}", ep.id),
        )
        .await?;
        Ok((ep, secret))
    }

    /// `RotateWebhookSecret` (`billing_admin`): the old secret still signs
    /// for [`ROTATION_OVERLAP`].
    pub async fn rotate_secret(
        &self,
        caller: &Caller,
        endpoint_id: &str,
    ) -> Result<(Endpoint, String)> {
        let mut ep = self
            .notes
            .endpoint(endpoint_id)
            .await?
            .ok_or_else(|| MeterError::not_found("webhook", endpoint_id))?;
        let c = self.customer_for_admin(caller, &ep.customer_id).await?;
        let secret = webhook::new_secret((self.random)());
        let now = (self.clock)();
        ep.previous = Some((ep.secret.clone(), now.add_nanos(nanos(ROTATION_OVERLAP))));
        ep.secret = self.sealer.seal(&secret)?;
        self.notes.put_endpoint(ep.clone()).await?;
        self.audit(
            caller,
            "meter.webhook.rotate",
            &c,
            format!("webhook:{}", ep.id),
        )
        .await?;
        Ok((ep, secret))
    }

    /// `CreateRule` (`billing_admin`).
    pub async fn create_rule(
        &self,
        caller: &Caller,
        customer_id: &str,
        feature: Option<&str>,
        thresholds: Vec<u32>,
        spend_cap: bool,
        channels: Vec<Channel>,
    ) -> Result<Rule> {
        let c = self.customer_for_admin(caller, customer_id).await?;
        if thresholds.is_empty() || thresholds.iter().any(|t| *t == 0 || *t > 1000) {
            return Err(MeterError::invalid(
                "thresholds",
                "1..=1000 percent, at least one",
            ));
        }
        let feature = feature
            .map(|f| FeatureKey::new(f).map_err(|e| MeterError::invalid("feature", e.to_string())))
            .transpose()?;
        let rule = Rule {
            id: new_id(),
            customer_id: c.id.clone(),
            feature,
            thresholds,
            spend_cap,
            channels,
        };
        self.notes.put_rule(rule.clone()).await?;
        self.audit(caller, "meter.rule.create", &c, format!("rule:{}", rule.id))
            .await?;
        Ok(rule)
    }

    /// `ListRules` (`billing_viewer`).
    pub async fn list_rules(&self, caller: &Caller, customer_id: &str) -> Result<Vec<Rule>> {
        let c = self
            .store
            .customer(customer_id)
            .await?
            .ok_or_else(|| MeterError::not_found("customer", customer_id))?;
        self.authz.require_billing_view(caller, &c.org).await?;
        self.notes.rules_of(&c.id).await
    }

    /// `ListDeliveries` (`billing_viewer`): the newest first.
    pub async fn list_deliveries(
        &self,
        caller: &Caller,
        customer_id: &str,
        limit: usize,
    ) -> Result<Vec<Delivery>> {
        let c = self
            .store
            .customer(customer_id)
            .await?
            .ok_or_else(|| MeterError::not_found("customer", customer_id))?;
        self.authz.require_billing_view(caller, &c.org).await?;
        self.notes.deliveries_of(&c.id, limit.clamp(1, 500)).await
    }
}
