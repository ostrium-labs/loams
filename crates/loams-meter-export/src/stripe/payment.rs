//! Payment state from Stripe (§63 §13, Q967, D2212): webhooks plus a daily
//! poll set a customer's state to `card` or `none`.
//!
//! **No card data passes through Loams.** Customers add a payment method on
//! Stripe's hosted pages (the Customer Portal, Checkout in setup mode). The
//! webhook body is read for ids only: the event's `type` and the customer id
//! (a payment method's `customer`, or a customer event's object id). Card
//! fields in a `payment_method.*` body are never deserialised, kept or
//! logged. The state then comes from the customer object alone: **`card`
//! when the customer has a default payment method**
//! (`invoice_settings.default_payment_method` or a legacy `default_source`),
//! which are ids, else `none`. Payment methods are never listed, since a list
//! carries brand, last four digits and expiry.
//!
//! **Who may set it.** The state goes to a [`PaymentStateSink`]: in the
//! `meter` role, a `CustomerService.SetPaymentState` client that
//! authenticates as the billing exporter's service identity, the one
//! principal the service accepts for that method (§63 §19 item 5). The
//! service maps the Stripe customer id to the Loams customer, audits the
//! change, and leaves an operator's `invoice` (contract) state alone: this
//! module never sends `invoice`.
//!
//! **Signatures.** `Stripe-Signature: t=<unix>,v1=<hex>[,v1=…]`, where `v1` is
//! `HMAC-SHA256(endpoint secret, "<t>.<raw body>")`. Several secrets may be
//! configured while one is rolled. Events older or newer than the tolerance
//! (300 s, Stripe's default) are refused, so a captured event cannot be
//! replayed later.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use sha2::Sha256;

use super::{EXPORTER, StripeConfig, stripe_message};
use crate::retry::{self, Reply};
use crate::{ExportError, short};

/// A customer's payment state as Stripe shows it. The meter's third state,
/// `invoice`, is set by an operator only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaymentState {
    /// No usable payment method on file.
    None,
    /// A default payment method on file.
    Card,
}

impl PaymentState {
    pub fn as_str(self) -> &'static str {
        match self {
            PaymentState::None => "none",
            PaymentState::Card => "card",
        }
    }
}

/// Where payment state goes: `CustomerService.SetPaymentState`, called as
/// the billing exporter's service identity (module docs).
#[async_trait]
pub trait PaymentStateSink: Send + Sync + fmt::Debug {
    async fn set_payment_state(
        &self,
        stripe_customer_id: &str,
        state: PaymentState,
    ) -> Result<(), ExportError>;
}

/// The default signature tolerance.
pub const SIGNATURE_TOLERANCE: Duration = Duration::from_secs(300);

/// Checks `Stripe-Signature` headers.
#[derive(Clone)]
pub struct WebhookVerifier {
    secrets: Vec<String>,
    tolerance: Duration,
}

impl fmt::Debug for WebhookVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebhookVerifier")
            .field("secrets", &self.secrets.len())
            .field("tolerance", &self.tolerance)
            .finish()
    }
}

impl WebhookVerifier {
    /// `whsec_…` endpoint secrets: the current one, and the old one while a
    /// roll is in progress.
    pub fn new(secrets: Vec<String>) -> Result<WebhookVerifier, ExportError> {
        let secrets: Vec<String> = secrets.into_iter().filter(|s| !s.is_empty()).collect();
        if secrets.is_empty() {
            return Err(ExportError::Invalid(
                "stripe: no webhook signing secret".into(),
            ));
        }
        Ok(WebhookVerifier {
            secrets,
            tolerance: SIGNATURE_TOLERANCE,
        })
    }

    pub fn with_tolerance(mut self, tolerance: Duration) -> WebhookVerifier {
        self.tolerance = tolerance;
        self
    }

    /// Verifies one delivery at `now` (Unix seconds).
    pub fn verify(&self, header: &str, body: &[u8], now: i64) -> Result<(), ExportError> {
        let bad = |why: &str| ExportError::Webhook(format!("Stripe-Signature: {why}"));
        let mut timestamp: Option<i64> = None;
        let mut signatures: Vec<Vec<u8>> = Vec::new();
        for item in header.split(',') {
            match item.trim().split_once('=') {
                Some(("t", v)) => timestamp = v.parse().ok(),
                Some(("v1", v)) => {
                    if let Ok(sig) = hex::decode(v) {
                        signatures.push(sig);
                    }
                }
                _ => {}
            }
        }
        let t = timestamp.ok_or_else(|| bad("no timestamp"))?;
        if signatures.is_empty() {
            return Err(bad("no v1 signature"));
        }
        let age = now.abs_diff(t);
        if age > self.tolerance.as_secs() {
            return Err(bad("outside the tolerance (a replay or a skewed clock)"));
        }
        for secret in &self.secrets {
            for sig in &signatures {
                let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes())
                    .map_err(|_| bad("unusable secret"))?;
                mac.update(t.to_string().as_bytes());
                mac.update(b".");
                mac.update(body);
                // Constant time.
                if mac.verify_slice(sig).is_ok() {
                    return Ok(());
                }
            }
        }
        Err(bad("no signature matches"))
    }
}

/// What a webhook delivery did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookOutcome {
    /// The customer's state was read from Stripe and passed on.
    Updated {
        stripe_customer_id: String,
        state: PaymentState,
    },
    /// A verified event this exporter does not act on.
    Ignored { event_type: String },
}

/// What a daily poll did.
#[derive(Debug, Default)]
pub struct PollReport {
    pub card: u32,
    pub none: u32,
    /// Customers whose state could not be read or set; the next poll retries.
    pub failed: Vec<(String, ExportError)>,
}

/// Payment state from Stripe: webhooks and the daily poll.
#[derive(Debug, Clone)]
pub struct StripePayments {
    http: reqwest::Client,
    cfg: StripeConfig,
    verifier: WebhookVerifier,
    sink: Arc<dyn PaymentStateSink>,
}

impl StripePayments {
    pub fn new(
        cfg: StripeConfig,
        verifier: WebhookVerifier,
        sink: Arc<dyn PaymentStateSink>,
    ) -> Result<StripePayments, ExportError> {
        if cfg.api_key.trim().is_empty() {
            return Err(ExportError::Invalid("stripe: no API key".into()));
        }
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(|e| ExportError::Invalid(format!("stripe: HTTP client: {e}")))?;
        Ok(StripePayments {
            http,
            cfg,
            verifier,
            sink,
        })
    }

    /// Handles one webhook delivery: `signature` is the `Stripe-Signature`
    /// header, `body` the raw body, `now` Unix seconds. The caller answers
    /// 2xx on `Ok` and 400 on a [`ExportError::Webhook`] (Stripe then
    /// retries, as it does on 5xx for the other errors).
    pub async fn handle_webhook(
        &self,
        signature: &str,
        body: &[u8],
        now: i64,
    ) -> Result<WebhookOutcome, ExportError> {
        self.verifier.verify(signature, body, now)?;
        let event: Event = serde_json::from_slice(body)
            .map_err(|e| ExportError::Webhook(format!("not a Stripe event: {e}")))?;
        let customer = match event.r#type.as_str() {
            "payment_method.attached" | "payment_method.updated" => event.data.object.customer,
            // Detached: the method no longer names the customer; it was in
            // `previous_attributes`.
            "payment_method.detached" => event
                .data
                .previous_attributes
                .and_then(|p| p.customer)
                .or(event.data.object.customer),
            "customer.created" | "customer.updated" | "customer.deleted" => event.data.object.id,
            _ => {
                return Ok(WebhookOutcome::Ignored {
                    event_type: event.r#type,
                });
            }
        };
        let Some(customer) = customer.filter(|c| valid_customer_id(c)) else {
            return Ok(WebhookOutcome::Ignored {
                event_type: event.r#type,
            });
        };
        // The event is a hint only: the state is read from the customer as
        // it is now, so out-of-order deliveries cannot leave a stale state.
        let state = self.refresh(&customer).await?;
        Ok(WebhookOutcome::Updated {
            stripe_customer_id: customer,
            state,
        })
    }

    /// The daily poll: reads each customer's state and passes it on, also
    /// when it did not change (a missed webhook is corrected within a day).
    pub async fn poll(&self, customers: &[String]) -> PollReport {
        let mut report = PollReport::default();
        for c in customers {
            match self.refresh(c).await {
                Ok(PaymentState::Card) => report.card += 1,
                Ok(PaymentState::None) => report.none += 1,
                Err(e) => report.failed.push((c.clone(), e)),
            }
        }
        report
    }

    /// Reads one customer's state and passes it to the sink.
    async fn refresh(&self, customer: &str) -> Result<PaymentState, ExportError> {
        if !valid_customer_id(customer) {
            return Err(ExportError::Invalid(format!(
                "{customer:?} is not a Stripe customer id"
            )));
        }
        let state = self.customer_state(customer).await?;
        self.sink.set_payment_state(customer, state).await?;
        Ok(state)
    }

    /// `GET /v1/customers/{id}`: `card` with a default payment method, `none`
    /// without one or for a deleted (or unknown) customer.
    pub async fn customer_state(&self, customer: &str) -> Result<PaymentState, ExportError> {
        if !valid_customer_id(customer) {
            return Err(ExportError::Invalid(format!(
                "{customer:?} is not a Stripe customer id"
            )));
        }
        let url = format!(
            "{}/v1/customers/{customer}",
            self.cfg.api_base.trim_end_matches('/')
        );
        let reply: Result<Reply, ExportError> =
            retry::send(EXPORTER, &self.cfg.retry, stripe_message, || {
                let mut req = self.http.get(&url).bearer_auth(&self.cfg.api_key);
                if let Some(v) = &self.cfg.api_version {
                    req = req.header("Stripe-Version", v);
                }
                req.send()
            })
            .await;
        let reply = match reply {
            Ok(r) => r,
            Err(ExportError::Rejected { status: 404, .. }) => return Ok(PaymentState::None),
            Err(e) => return Err(e),
        };
        let c: Customer = serde_json::from_str(&reply.body).map_err(|e| {
            ExportError::Invalid(format!(
                "stripe: customer {customer}: {}",
                short(&e.to_string())
            ))
        })?;
        let has_default = c
            .invoice_settings
            .and_then(|s| s.default_payment_method)
            .is_some_and(|v| !v.is_null())
            || c.default_source.is_some_and(|v| !v.is_null());
        Ok(if c.deleted || !has_default {
            PaymentState::None
        } else {
            PaymentState::Card
        })
    }
}

/// `cus_` and Stripe's id characters, so it is safe in a URL path.
pub fn valid_customer_id(id: &str) -> bool {
    id.len() <= 255
        && id
            .strip_prefix("cus_")
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

// The few fields read from a webhook event; serde skips everything else
// (card details included) without keeping it.
#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    r#type: String,
    data: EventData,
}

#[derive(Deserialize)]
struct EventData {
    object: Ids,
    #[serde(default)]
    previous_attributes: Option<Ids>,
}

#[derive(Deserialize)]
struct Ids {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, deserialize_with = "customer_ref")]
    customer: Option<String>,
}

/// `customer` is an id, or an object when expanded, or null.
fn customer_ref<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(match v {
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Object(o)) => {
            o.get("id").and_then(|i| i.as_str()).map(str::to_owned)
        }
        _ => None,
    })
}

#[derive(Deserialize)]
struct Customer {
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    invoice_settings: Option<InvoiceSettings>,
    #[serde(default)]
    default_source: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct InvoiceSettings {
    #[serde(default)]
    default_payment_method: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(secret: &str, t: i64, body: &[u8]) -> String {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).expect("key");
        mac.update(format!("{t}.").as_bytes());
        mac.update(body);
        format!("t={t},v1={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn verifies_signature_tolerance_and_rolled_secrets() {
        let v = WebhookVerifier::new(vec!["whsec_new".into(), "whsec_old".into()]).expect("v");
        let body = br#"{"type":"customer.updated"}"#;
        assert!(v.verify(&sign("whsec_new", 1000, body), body, 1000).is_ok());
        assert!(
            v.verify(&sign("whsec_old", 1000, body), body, 1200).is_ok(),
            "old secret during a roll"
        );
        assert!(
            v.verify(&sign("whsec_other", 1000, body), body, 1000)
                .is_err(),
            "wrong secret"
        );
        assert!(
            v.verify(&sign("whsec_new", 1000, body), body, 1301)
                .is_err(),
            "too old: a replay"
        );
        assert!(
            v.verify(&sign("whsec_new", 1000, body), b"{}", 1000)
                .is_err(),
            "body changed"
        );
        assert!(v.verify("v1=00", body, 1000).is_err(), "no timestamp");
        assert!(v.verify("t=1000", body, 1000).is_err(), "no signature");
        // A second v1 (Stripe sends one per active secret) may be the match.
        let two = format!("{},v1=deadbeef", sign("whsec_new", 1000, body));
        assert!(v.verify(&two, body, 1000).is_ok());
    }

    #[test]
    fn customer_ids_are_path_safe() {
        assert!(valid_customer_id("cus_ABC123"));
        for bad in ["cus_", "pm_1", "cus_../x", "cus_a/b", "cus_a?b", ""] {
            assert!(!valid_customer_id(bad), "{bad}");
        }
    }

    #[test]
    fn no_secret_is_refused_and_debug_hides_secrets() {
        assert!(WebhookVerifier::new(vec![String::new()]).is_err());
        let v = WebhookVerifier::new(vec!["whsec_x".into()]).expect("v");
        assert!(!format!("{v:?}").contains("whsec_x"));
    }
}
