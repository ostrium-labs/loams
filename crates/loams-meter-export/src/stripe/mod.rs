//! The Stripe exporter (§63 §16, Q965: the first exporter; plan ME1 Task 24).
//!
//! **What it sends.** For each closed **hour** of a billable feature, the
//! hour's **overage**, not its raw usage (platform PD7):
//!
//! ```text
//! overage(h) = O(h) − O(h−1),   O(h) = max(0, cum(h) − included)
//! ```
//!
//! where `cum(h)` is the period's usage through hour `h` (from
//! `_meter.windows`, hour windows) and `included` is what the grants gave at
//! the period's start. With [`StripeConfig::whole_units`] (the default) `O(h)`
//! is rounded up to a whole unit first, so every hour's value is a whole
//! number and the hours still add up to the period's rounded-up overage.
//! One meter event per hour: `event_name` (the Stripe meter), `payload
//! [stripe_customer_id]`, `payload[value]`, `timestamp` = the hour. An hour
//! with no overage is sent only to overwrite an earlier non-zero send with 0.
//!
//! **Idempotency (D2211).** The Stripe meters are hour pre-aggregated
//! (`event_time_window = hour`): the latest event of an hour overwrites the
//! earlier ones. The `identifier` is `loams:<meter>:<subject>:<hour>:<rev>`,
//! where `<hour>` is the hour's Unix seconds and `<rev>` is the cumulative's
//! revision: the highest `close_run` of the period's hours through `h`,
//! because a re-closed earlier hour changes every later hour's overage. A
//! re-closed hour is therefore sent again under a new identifier and replaces
//! the earlier value (the adjustment). If the same revision must carry a
//! different value (the included amount changed), `-<n>` is appended. An
//! identifier over Stripe's 100 characters becomes `loams:h:<sha256 hex>` of
//! the long form. The [`ExportLedger`] holds the planned send before the
//! request, so a retry after a timeout or a crash sends the same identifier
//! and `Idempotency-Key`; Stripe keeps identifiers unique for at least 24 h.
//!
//! **Periods.** Billing periods start on whole UTC hours
//! (`billing_cycle_anchor` at the next whole hour, never reset on plan
//! changes), so no hour straddles a period start. An off-hour period start
//! stops that customer's export with [`ExportError::OffHourPeriod`]; the
//! meter alerts.
//!
//! **Payment state** is [`payment`]: webhooks and a daily poll. No card data
//! passes through Loams.

pub mod payment;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::ledger::{ExportLedger, LedgerEntry, LedgerKey};
use crate::retry::{self, RetryPolicy};
use crate::{ExportError, Quantity};

/// The ledger's exporter name.
pub const EXPORTER: &str = "stripe";
/// Stripe's live API.
pub const API_BASE: &str = "https://api.stripe.com";
/// Stripe's longest meter event identifier.
pub const MAX_IDENTIFIER: usize = 100;
const HOUR: i64 = 3600;

/// The exporter's settings.
#[derive(Clone)]
pub struct StripeConfig {
    /// `https://api.stripe.com`, or a test server.
    pub api_base: String,
    /// A restricted key with write access to meter events and read access to
    /// customers; nothing else is needed.
    pub api_key: String,
    /// `Stripe-Version`, when set; otherwise the account's default.
    pub api_version: Option<String>,
    /// Round each hour's cumulative overage up to whole units (see the
    /// module docs). Loams' units (ms, bytes, tokens) are whole already.
    pub whole_units: bool,
    pub retry: RetryPolicy,
    /// Per request.
    pub timeout: Duration,
}

impl StripeConfig {
    pub fn new(api_key: impl Into<String>) -> StripeConfig {
        StripeConfig {
            api_base: API_BASE.to_owned(),
            api_key: api_key.into(),
            api_version: None,
            whole_units: true,
            retry: RetryPolicy::default(),
            timeout: Duration::from_secs(30),
        }
    }
}

impl fmt::Debug for StripeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StripeConfig")
            .field("api_base", &self.api_base)
            .field("api_key", &"<redacted>")
            .field("api_version", &self.api_version)
            .field("whole_units", &self.whole_units)
            .field("retry", &self.retry)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// One closed hour window of a billable feature (`_meter.windows`,
/// `window_size = hour`, read with `FINAL`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HourUsage {
    /// The hour's start, Unix seconds UTC (a multiple of 3600).
    pub hour_start: i64,
    pub value: Quantity,
    /// The window row's version (`close_run`).
    pub close_run: u64,
}

/// One customer's period of one feature, to export.
#[derive(Debug, Clone)]
pub struct PeriodExport {
    /// The Stripe meter's `event_name` (by convention the Loams meter slug).
    pub event_name: String,
    /// The billing subject (`org_<id>`).
    pub subject: String,
    /// `cus_…`.
    pub stripe_customer_id: String,
    /// The period's start, Unix seconds UTC.
    pub period_start: i64,
    /// What the grants gave at the period's start, in the feature's unit.
    pub included: Quantity,
    /// Every closed hour of the period so far, in any order.
    pub hours: Vec<HourUsage>,
}

/// One hour's overage and the revision it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HourOverage {
    pub hour_start: i64,
    pub value: Quantity,
    pub revision: u64,
}

/// The overage of each closed hour (module docs). Hours before the period
/// start are ignored; a missing hour counts as no usage.
pub fn hourly_overage(
    period_start: i64,
    included: Quantity,
    hours: &[HourUsage],
    whole_units: bool,
) -> Vec<HourOverage> {
    let mut sorted: Vec<&HourUsage> = hours
        .iter()
        .filter(|h| h.hour_start >= period_start)
        .collect();
    sorted.sort_by_key(|h| h.hour_start);
    let mut cum = Quantity::ZERO;
    let mut prev_over = Quantity::ZERO;
    let mut revision = 0;
    let mut out = Vec::with_capacity(sorted.len());
    for h in sorted {
        cum = cum.saturating_add(h.value);
        revision = revision.max(h.close_run);
        let mut over = cum.saturating_sub(included);
        if over.is_negative() {
            over = Quantity::ZERO;
        }
        if whole_units {
            over = over.ceil_units();
        }
        out.push(HourOverage {
            hour_start: h.hour_start,
            value: over.saturating_sub(prev_over),
            revision,
        });
        prev_over = over;
    }
    out
}

/// The identifier of one send (module docs).
pub fn identifier(event_name: &str, subject: &str, hour: i64, revision: u64, seq: u32) -> String {
    let mut id = format!("loams:{event_name}:{subject}:{hour}:{revision}");
    if seq > 0 {
        id.push_str(&format!("-{seq}"));
    }
    if id.len() > MAX_IDENTIFIER {
        id = format!("loams:h:{}", hex::encode(Sha256::digest(id.as_bytes())));
    }
    id
}

/// What one export run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportReport {
    /// Meter events Stripe accepted in this run.
    pub sent: u32,
    /// Hours already sent with the same value, or with no overage to send.
    pub unchanged: u32,
}

/// Sends hourly overage to Stripe.
#[derive(Debug, Clone)]
pub struct StripeExporter {
    http: reqwest::Client,
    cfg: StripeConfig,
    ledger: Arc<dyn ExportLedger>,
}

impl StripeExporter {
    pub fn new(
        cfg: StripeConfig,
        ledger: Arc<dyn ExportLedger>,
    ) -> Result<StripeExporter, ExportError> {
        if cfg.api_key.trim().is_empty() {
            return Err(ExportError::Invalid("stripe: no API key".into()));
        }
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(|e| ExportError::Invalid(format!("stripe: HTTP client: {e}")))?;
        Ok(StripeExporter { http, cfg, ledger })
    }

    /// Exports one customer's period: every closed hour whose overage
    /// differs from what Stripe last accepted, and any send a crash or a
    /// failed run left unaccepted. Stops at the first error; the next run
    /// continues where it stopped.
    pub async fn export_period(&self, p: &PeriodExport) -> Result<ExportReport, ExportError> {
        if p.period_start.rem_euclid(HOUR) != 0 {
            return Err(ExportError::OffHourPeriod {
                subject: p.subject.clone(),
                start: p.period_start,
            });
        }
        if !p.stripe_customer_id.starts_with("cus_") {
            return Err(ExportError::Invalid(format!(
                "stripe: {} has no Stripe customer id",
                p.subject
            )));
        }
        if let Some(h) = p.hours.iter().find(|h| h.hour_start.rem_euclid(HOUR) != 0) {
            return Err(ExportError::Invalid(format!(
                "stripe: window {} of {} is not an hour window",
                h.hour_start, p.subject
            )));
        }
        let mut report = ExportReport::default();
        for o in hourly_overage(p.period_start, p.included, &p.hours, self.cfg.whole_units) {
            let key = LedgerKey {
                exporter: EXPORTER.to_owned(),
                meter: p.event_name.clone(),
                subject: p.subject.clone(),
                window_start: o.hour_start,
            };
            let prev = self.ledger.get(&key).await?;
            let entry = match prev {
                // Accepted already with this value: nothing to do.
                Some(e) if e.value == o.value && e.sent => {
                    report.unchanged += 1;
                    continue;
                }
                // Planned and not accepted (a failed run or a crash): the
                // same identifier again.
                Some(e) if e.value == o.value => e,
                // Never sent and nothing to overwrite.
                None if o.value.is_zero() => {
                    report.unchanged += 1;
                    continue;
                }
                prev => {
                    let mut seq = 0;
                    let mut id = identifier(&p.event_name, &p.subject, o.hour_start, o.revision, 0);
                    if let Some(e) = &prev
                        && e.revision == o.revision
                    {
                        seq = e.seq + 1;
                        id = identifier(&p.event_name, &p.subject, o.hour_start, o.revision, seq);
                    }
                    let entry = LedgerEntry {
                        identifier: id,
                        value: o.value,
                        total: o.value,
                        revision: o.revision,
                        seq,
                        sent: false,
                    };
                    self.ledger.put(&key, &entry).await?;
                    entry
                }
            };
            self.send_meter_event(p, o.hour_start, &entry).await?;
            let done = LedgerEntry {
                sent: true,
                ..entry
            };
            self.ledger.put(&key, &done).await?;
            report.sent += 1;
        }
        Ok(report)
    }

    async fn send_meter_event(
        &self,
        p: &PeriodExport,
        hour: i64,
        entry: &LedgerEntry,
    ) -> Result<(), ExportError> {
        let url = format!(
            "{}/v1/billing/meter_events",
            self.cfg.api_base.trim_end_matches('/')
        );
        let value = entry.value.to_string();
        let timestamp = hour.to_string();
        let form = [
            ("event_name", p.event_name.as_str()),
            ("payload[stripe_customer_id]", p.stripe_customer_id.as_str()),
            ("payload[value]", value.as_str()),
            ("timestamp", timestamp.as_str()),
            ("identifier", entry.identifier.as_str()),
        ];
        retry::send(EXPORTER, &self.cfg.retry, stripe_message, || {
            let mut req = self
                .http
                .post(&url)
                .bearer_auth(&self.cfg.api_key)
                .header("Idempotency-Key", &entry.identifier)
                .form(&form);
            if let Some(v) = &self.cfg.api_version {
                req = req.header("Stripe-Version", v);
            }
            req.send()
        })
        .await?;
        tracing::debug!(identifier = %entry.identifier, value = %entry.value, "stripe: meter event accepted");
        Ok(())
    }
}

/// Stripe's error message from an error body (`{"error": {"message": …}}`),
/// or the status alone; never the request.
pub(crate) fn stripe_message(body: &str) -> String {
    #[derive(Deserialize)]
    struct Body {
        error: Inner,
    }
    #[derive(Deserialize)]
    struct Inner {
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        message: Option<String>,
    }
    match serde_json::from_str::<Body>(body) {
        Ok(b) => match (b.error.code, b.error.message) {
            (Some(c), Some(m)) => format!("{c}: {m}"),
            (Some(c), None) => c,
            (None, Some(m)) => m,
            (None, None) => "no message".into(),
        },
        Err(_) => "no Stripe error body".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: i64, v: i64, run: u64) -> HourUsage {
        HourUsage {
            hour_start: n * HOUR,
            value: Quantity::from_units(v),
            close_run: run,
        }
    }

    fn values(o: &[HourOverage]) -> Vec<i64> {
        o.iter()
            .map(|x| (x.value.raw() / Quantity::SCALE) as i64)
            .collect()
    }

    #[test]
    fn overage_only_above_included() {
        let o = hourly_overage(
            0,
            Quantity::from_units(100),
            &[h(0, 60, 1), h(1, 30, 1), h(2, 40, 1), h(3, 5, 1)],
            true,
        );
        assert_eq!(values(&o), vec![0, 0, 30, 5]);
        // The hours add up to the period's overage.
        let total: i128 = o.iter().map(|x| x.value.raw()).sum();
        assert_eq!(total, Quantity::from_units(35).raw());
    }

    #[test]
    fn overage_order_independent_and_skips_hours_before_the_period() {
        let o = hourly_overage(
            2 * HOUR,
            Quantity::ZERO,
            &[h(3, 2, 1), h(1, 9, 1), h(2, 1, 1)],
            true,
        );
        assert_eq!(
            o.iter().map(|x| x.hour_start).collect::<Vec<_>>(),
            vec![2 * HOUR, 3 * HOUR]
        );
        assert_eq!(values(&o), vec![1, 2]);
    }

    #[test]
    fn revision_is_the_highest_close_run_so_far() {
        let o = hourly_overage(
            0,
            Quantity::ZERO,
            &[h(0, 1, 7), h(1, 1, 3), h(2, 1, 9)],
            true,
        );
        assert_eq!(
            o.iter().map(|x| x.revision).collect::<Vec<_>>(),
            vec![7, 7, 9]
        );
    }

    #[test]
    fn whole_units_round_the_cumulative_up() {
        let q = |s: &str| Quantity::parse(s).expect("q");
        let hours = [
            HourUsage {
                hour_start: 0,
                value: q("0.4"),
                close_run: 1,
            },
            HourUsage {
                hour_start: HOUR,
                value: q("0.4"),
                close_run: 1,
            },
            HourUsage {
                hour_start: 2 * HOUR,
                value: q("0.4"),
                close_run: 1,
            },
        ];
        let o = hourly_overage(0, Quantity::ZERO, &hours, true);
        // O = ceil(0.4, 0.8, 1.2) = 1, 1, 2.
        assert_eq!(values(&o), vec![1, 0, 1]);
        let exact = hourly_overage(0, Quantity::ZERO, &hours, false);
        assert!(exact.iter().all(|x| x.value == q("0.4")));
    }

    #[test]
    fn identifier_form_and_length() {
        assert_eq!(
            identifier("house_cpu_ms", "org_42", 3600, 7, 0),
            "loams:house_cpu_ms:org_42:3600:7"
        );
        assert_eq!(
            identifier("house_cpu_ms", "org_42", 3600, 7, 2),
            "loams:house_cpu_ms:org_42:3600:7-2"
        );
        let long = identifier("m", &"s".repeat(200), 0, 1, 0);
        assert!(
            long.starts_with("loams:h:") && long.len() <= MAX_IDENTIFIER,
            "{long}"
        );
        assert_eq!(long, identifier("m", &"s".repeat(200), 0, 1, 0), "stable");
    }

    #[test]
    fn stripe_message_reads_the_error_body() {
        assert_eq!(
            stripe_message(r#"{"error":{"code":"resource_missing","message":"No such customer"}}"#),
            "resource_missing: No such customer"
        );
        assert_eq!(stripe_message("<html>"), "no Stripe error body");
    }

    #[test]
    fn config_debug_hides_the_key() {
        let c = StripeConfig::new("sk_test_secret");
        assert!(!format!("{c:?}").contains("sk_test_secret"));
    }
}
