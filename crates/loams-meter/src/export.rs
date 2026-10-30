//! The `meter` role's side of ME-export's exporters (design §63 §16; ME1
//! role wiring, part 3, D2207), behind the features `export-stripe`,
//! `export-openmeter`, `export-lago` and `export-webhook`:
//!
//! - [`DocExports`] is the exporters' ledger and cursors
//!   ([`ExportLedger`], [`ExportCursors`]) in the role's control store: one
//!   versioned document (`meter-exports`), written with compare-and-set so
//!   every instance shares it. Ledger entries of windows older than
//!   [`LEDGER_RETAIN`] are dropped on write.
//! - [`closed_hour_windows`] reads closed hour windows from
//!   `_meter.windows FINAL`, the input of the window exporters.
//! - [`ExportSchedule`] runs the configured window exporters every
//!   [`EXPORT_EVERY`] over the hours closed in the last [`LOOKBACK`]
//!   (`export-webhook` today), and the daily Stripe payment-state poll
//!   (`export-stripe`).
//! - [`CustomerPayments`] passes Stripe's payment state to
//!   `CustomerService.SetPaymentState` as the billing exporter's service
//!   identity, and [`stripe_webhook_router`] serves
//!   `POST /v1/meter/webhooks/stripe`.
//!
//! - The scheduled exports of D2403, each on when its exporter is
//!   configured, every [`EXPORT_EVERY`] with the window exporters:
//!   - Stripe's period export ([`stripe_periods`]): each customer with a
//!     Stripe id, each metered `SUM` or `COUNT` feature of its subjects,
//!     the current period (and the previous one for [`LOOKBACK`] after it
//!     ends), its closed hours from `_meter.windows FINAL` and what the
//!     grants gave at the period's start;
//!   - Lago ([`lago_windows`]): the closed hour windows of the last
//!     [`LOOKBACK`], each with its customer's subscription and the meter's
//!     billable-metric code;
//!   - OpenMeter's raw events ([`ExportSchedule::openmeter_once`]): each
//!     `_system/_meter` partition this node owns, read from the exporter's
//!     cursor (from the start of the retained stream the first time).
//!
//!   The names each provider needs come from [`ExportMapping`]
//!   (`LOAMS_METER_EXPORT_MAPPING_FILE`), with defaults: the Stripe meter's
//!   `event_name` and the Lago billable metric's `code` are the meter's
//!   slug, and the Lago `external_subscription_id` is the Loams customer's
//!   id.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_meter_export::{
    ExportCursors, ExportError, ExportLedger, LedgerEntry, LedgerKey, Quantity, WindowRow,
    WindowSize,
};
use loams_meter_house::HouseSql;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::customers::Clock;
use crate::store::{Records, VersionedDoc};

pub use loams_meter_export as exporters;

/// How long ledger entries are kept after their window.
pub const LEDGER_RETAIN: Duration = Duration::from_secs(45 * 24 * 3600);

/// How often the window exporters run.
pub const EXPORT_EVERY: Duration = Duration::from_secs(5 * 60);

/// How far back each run looks for closed hours (late events re-close
/// windows for up to 24 hours, §63 §8.2).
pub const LOOKBACK: Duration = Duration::from_secs(26 * 3600);

/// How often Stripe's payment state is polled.
pub const POLL_EVERY: Duration = Duration::from_secs(24 * 3600);

const MAX_WRITE_ATTEMPTS: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LedgerRow {
    exporter: String,
    meter: String,
    subject: String,
    window_start: i64,
    identifier: String,
    value: String,
    total: String,
    revision: u64,
    seq: u32,
    sent: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ExportsSnapshot {
    #[serde(default)]
    ledger: Vec<LedgerRow>,
    /// (exporter, partition, next offset).
    #[serde(default)]
    cursors: Vec<(String, u32, u64)>,
}

#[derive(Debug, Default)]
struct Exports {
    ledger: BTreeMap<LedgerKey, LedgerEntry>,
    cursors: BTreeMap<(String, u32), u64>,
}

fn ledger_err(e: impl fmt::Display) -> ExportError {
    ExportError::Ledger(e.to_string())
}

impl Exports {
    fn from_snapshot(s: ExportsSnapshot) -> Result<Exports, ExportError> {
        let mut ledger = BTreeMap::new();
        for r in s.ledger {
            ledger.insert(
                LedgerKey {
                    exporter: r.exporter,
                    meter: r.meter,
                    subject: r.subject,
                    window_start: r.window_start,
                },
                LedgerEntry {
                    identifier: r.identifier,
                    value: Quantity::parse(&r.value).map_err(ledger_err)?,
                    total: Quantity::parse(&r.total).map_err(ledger_err)?,
                    revision: r.revision,
                    seq: r.seq,
                    sent: r.sent,
                },
            );
        }
        Ok(Exports {
            ledger,
            cursors: s.cursors.into_iter().map(|(e, p, o)| ((e, p), o)).collect(),
        })
    }

    fn snapshot(&self) -> ExportsSnapshot {
        ExportsSnapshot {
            ledger: self
                .ledger
                .iter()
                .map(|(k, e)| LedgerRow {
                    exporter: k.exporter.clone(),
                    meter: k.meter.clone(),
                    subject: k.subject.clone(),
                    window_start: k.window_start,
                    identifier: e.identifier.clone(),
                    value: e.value.to_string(),
                    total: e.total.to_string(),
                    revision: e.revision,
                    seq: e.seq,
                    sent: e.sent,
                })
                .collect(),
            cursors: self
                .cursors
                .iter()
                .map(|((e, p), o)| (e.clone(), *p, *o))
                .collect(),
        }
    }
}

/// The exporters' ledger and cursors in one versioned document.
pub struct DocExports {
    doc: Arc<dyn VersionedDoc>,
    writes: tokio::sync::Mutex<()>,
    clock: Clock,
}

impl fmt::Debug for DocExports {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocExports").finish_non_exhaustive()
    }
}

impl DocExports {
    pub fn new(doc: Arc<dyn VersionedDoc>, clock: Clock) -> Self {
        DocExports {
            doc,
            writes: tokio::sync::Mutex::new(()),
            clock,
        }
    }

    async fn current(&self) -> Result<(Option<u64>, Exports), ExportError> {
        match self.doc.load().await.map_err(ledger_err)? {
            None => Ok((None, Exports::default())),
            Some((version, bytes)) => {
                let snap: ExportsSnapshot = serde_json::from_slice(&bytes).map_err(ledger_err)?;
                Ok((Some(version), Exports::from_snapshot(snap)?))
            }
        }
    }

    async fn write(&self, f: impl Fn(&mut Exports)) -> Result<(), ExportError> {
        let _one_writer_per_process = self.writes.lock().await;
        let oldest = (self.clock)()
            .0
            .div_euclid(1_000_000_000)
            .saturating_sub(i64::try_from(LEDGER_RETAIN.as_secs()).unwrap_or(i64::MAX));
        for _ in 0..MAX_WRITE_ATTEMPTS {
            let (version, mut exports) = self.current().await?;
            f(&mut exports);
            exports.ledger.retain(|k, _| k.window_start >= oldest);
            let bytes = serde_json::to_vec(&exports.snapshot()).map_err(ledger_err)?;
            if self
                .doc
                .save(version, bytes)
                .await
                .map_err(ledger_err)?
                .is_some()
            {
                return Ok(());
            }
        }
        Err(ExportError::Ledger(
            "the export ledger changed concurrently too often".into(),
        ))
    }
}

#[async_trait]
impl ExportLedger for DocExports {
    async fn get(&self, key: &LedgerKey) -> Result<Option<LedgerEntry>, ExportError> {
        Ok(self.current().await?.1.ledger.get(key).cloned())
    }

    async fn put(&self, key: &LedgerKey, entry: &LedgerEntry) -> Result<(), ExportError> {
        self.write(|e| {
            e.ledger.insert(key.clone(), entry.clone());
        })
        .await
    }
}

#[async_trait]
impl ExportCursors for DocExports {
    async fn get(&self, exporter: &str, partition: u32) -> Result<Option<u64>, ExportError> {
        Ok(self
            .current()
            .await?
            .1
            .cursors
            .get(&(exporter.to_string(), partition))
            .copied())
    }

    async fn put(
        &self,
        exporter: &str,
        partition: u32,
        next_offset: u64,
    ) -> Result<(), ExportError> {
        self.write(|e| {
            e.cursors
                .insert((exporter.to_string(), partition), next_offset);
        })
        .await
    }
}

/// The closed hour windows that started at or after `since` (Unix
/// seconds), from `_meter.windows FINAL`.
pub fn hour_windows_sql(since: i64) -> String {
    format!(
        "SELECT meter, subject, toUnixTimestamp(window_start), group_key, value, events, \
         close_run FROM {}.windows FINAL WHERE window_size = 'hour' AND window_start >= {} \
         ORDER BY meter, subject, window_start, group_key FORMAT TabSeparated",
        loams_meter_house::sql::DATABASE,
        loams_meter_house::sql::at(since)
    )
}

/// One `TabSeparated` field, unescaped.
fn unescape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Parses [`hour_windows_sql`]'s answer.
pub fn parse_hour_windows(tsv: &str) -> Result<Vec<WindowRow>, ExportError> {
    let bad = |line: &str, what: &str| {
        ExportError::Invalid(format!("a _meter.windows row ({what}): {line:?}"))
    };
    let mut out = Vec::new();
    for line in tsv.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 7 {
            return Err(bad(line, "field count"));
        }
        out.push(WindowRow {
            meter: unescape(f[0]),
            subject: unescape(f[1]),
            window_size: WindowSize::Hour,
            window_start: f[2].parse().map_err(|_| bad(line, "window_start"))?,
            group_key: unescape(f[3]),
            value: Quantity::parse(f[4]).map_err(|_| bad(line, "value"))?,
            events: f[5].parse().map_err(|_| bad(line, "events"))?,
            close_run: f[6].parse().map_err(|_| bad(line, "close_run"))?,
        });
    }
    Ok(out)
}

/// The closed hour windows since `since`.
pub async fn closed_hour_windows<H: HouseSql>(
    house: &H,
    since: i64,
) -> Result<Vec<WindowRow>, ExportError> {
    let tsv = house
        .query(&hour_windows_sql(since))
        .await
        .map_err(|e| ExportError::Invalid(format!("House: {e}")))?;
    parse_hour_windows(&tsv)
}

/// The provider names the scheduled exports need, per feature, meter and
/// customer (D2403). Every map is optional: what it does not name gets the
/// default. Read from YAML or JSON (`LOAMS_METER_EXPORT_MAPPING_FILE`):
///
/// ```yaml
/// stripe:
///   features: [house_cpu]          # empty: every SUM or COUNT feature
///   event_names: {house_cpu: loams_house_cpu}   # default: the meter's slug
/// lago:
///   meters: [house_cpu_ms]         # empty: every meter
///   codes: {house_cpu_ms: house_cpu}            # default: the meter's slug
///   subscriptions: {01JC...: sub_42, org_7: sub_7}  # by customer id or
///                                  # subject; default: the customer's id
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportMapping {
    #[serde(default)]
    pub stripe: StripeMapping,
    #[serde(default)]
    pub lago: LagoMapping,
}

/// Stripe's names (D2403).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StripeMapping {
    /// The features to export; empty: every metered `SUM` or `COUNT`
    /// feature.
    #[serde(default)]
    pub features: Vec<String>,
    /// Feature key → the Stripe meter's `event_name`; default: the
    /// feature's meter slug.
    #[serde(default)]
    pub event_names: BTreeMap<String, String>,
}

/// Lago's names (D2403).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LagoMapping {
    /// The meters to export; empty: every meter.
    #[serde(default)]
    pub meters: Vec<String>,
    /// Meter slug → the billable metric's `code`; default: the slug.
    #[serde(default)]
    pub codes: BTreeMap<String, String>,
    /// Customer id or subject → `external_subscription_id`; default: the
    /// customer's id.
    #[serde(default)]
    pub subscriptions: BTreeMap<String, String>,
}

impl ExportMapping {
    /// Parses YAML (JSON is YAML too).
    pub fn parse(text: &str) -> Result<ExportMapping, ExportError> {
        if text.trim().is_empty() {
            return Ok(ExportMapping::default());
        }
        serde_yaml_ng::from_str(text)
            .map_err(|e| ExportError::Invalid(format!("the export mapping: {e}")))
    }

    /// The Stripe meter `event_name` of `feature`, counting `meter`.
    pub fn stripe_event_name(&self, feature: &str, meter: &str) -> Option<String> {
        if !self.stripe.features.is_empty() && !self.stripe.features.iter().any(|f| f == feature) {
            return None;
        }
        Some(
            self.stripe
                .event_names
                .get(feature)
                .cloned()
                .unwrap_or_else(|| meter.to_string()),
        )
    }

    /// The Lago `code` of `meter`, or `None` when it is not exported.
    pub fn lago_code(&self, meter: &str) -> Option<String> {
        if !self.lago.meters.is_empty() && !self.lago.meters.iter().any(|m| m == meter) {
            return None;
        }
        Some(
            self.lago
                .codes
                .get(meter)
                .cloned()
                .unwrap_or_else(|| meter.to_string()),
        )
    }

    /// The Lago subscription of `customer_id` (for `subject`).
    pub fn lago_subscription(&self, customer_id: &str, subject: &str) -> String {
        self.lago
            .subscriptions
            .get(customer_id)
            .or_else(|| self.lago.subscriptions.get(subject))
            .cloned()
            .unwrap_or_else(|| customer_id.to_string())
    }
}

/// One Stripe period to export: the exporter's input but the hours, and
/// what selects the hours.
#[derive(Debug, Clone, PartialEq)]
pub struct StripePeriod {
    pub event_name: String,
    pub subject: String,
    pub stripe_customer_id: String,
    /// `[start, end)`, Unix seconds.
    pub period: (i64, i64),
    pub included: Quantity,
    /// The feature (its meter and group-by filter).
    pub feature: loams_meter_house::sql::FeatureDef,
}

/// The Stripe periods to export at `now` (module docs, D2403).
pub fn stripe_periods(records: &Records, mapping: &ExportMapping, now: i64) -> Vec<StripePeriod> {
    use loams_meter_core::balance::balance;
    use loams_meter_core::{AsOf, EntitlementKind, UtcNanos};
    let lookback = i64::try_from(LOOKBACK.as_secs()).unwrap_or(0);
    let mut out = Vec::new();
    for c in &records.customers {
        let Some(stripe_id) = c.stripe_customer_id.as_deref() else {
            continue;
        };
        for e in records
            .entitlements
            .iter()
            .filter(|e| c.subjects.contains(&e.subject))
        {
            let EntitlementKind::Metered { usage_period, .. } = &e.kind else {
                continue;
            };
            let Some(feature) = records
                .features
                .iter()
                .find(|f| f.key == e.feature && !f.archived)
            else {
                continue;
            };
            let Some(meter) = feature
                .meter
                .as_ref()
                .and_then(|slug| records.meters.iter().find(|m| &m.slug == slug))
            else {
                continue;
            };
            if !matches!(
                meter.aggregation,
                loams_meter_core::Aggregation::Sum | loams_meter_core::Aggregation::Count
            ) || feature.unit == loams_meter_core::Unit::ByteHours
            {
                continue;
            }
            let Some(event_name) =
                mapping.stripe_event_name(feature.key.as_str(), meter.slug.as_str())
            else {
                continue;
            };
            let grants: Vec<loams_meter_core::Grant> = records
                .grants
                .iter()
                .filter(|g| g.entitlement == e.id)
                .cloned()
                .collect();
            for (start, end) in loams_meter_house::period::periods_for(
                usage_period,
                UtcNanos(now * UtcNanos::SECOND),
                1,
            ) {
                if start > now || end + lookback < now {
                    continue;
                }
                let included = match balance(
                    e,
                    &grants,
                    &[],
                    UtcNanos(start * UtcNanos::SECOND),
                    AsOf::Replay,
                ) {
                    Ok(b) => Quantity::from_raw(b.total_available.raw().max(0)),
                    Err(err) => {
                        tracing::warn!(entitlement = e.id.as_str(), %err, "no included amount; period not exported");
                        continue;
                    }
                };
                out.push(StripePeriod {
                    event_name: event_name.clone(),
                    subject: e.subject.as_str().to_string(),
                    stripe_customer_id: stripe_id.to_string(),
                    period: (start, end),
                    included,
                    feature: loams_meter_house::period::feature_def(feature, meter),
                });
            }
        }
    }
    out
}

/// The closed hours of `p`'s feature for its subject, summed over the
/// feature's groups: `hour_start\tvalue\tclose_run` rows.
pub fn period_hours_sql(p: &StripePeriod, now: i64) -> String {
    use loams_meter_house::sql::{DATABASE, at, quote};
    let filter: String = p
        .feature
        .filter
        .iter()
        .map(|(k, v)| {
            format!(
                " AND JSONExtractString(group_key, {}) = {}",
                quote(k),
                quote(v)
            )
        })
        .collect();
    format!(
        "SELECT toUnixTimestamp(window_start), sum(value), max(close_run) FROM {DATABASE}.windows \
         FINAL WHERE window_size = 'hour' AND meter = {} AND subject = {} AND window_start >= {} \
         AND window_start < {}{filter} GROUP BY window_start ORDER BY window_start \
         FORMAT TabSeparated",
        quote(&p.feature.meter.slug),
        quote(&p.subject),
        at(p.period.0),
        at(p.period.1.min(now)),
    )
}

/// Parses [`period_hours_sql`]'s answer.
#[cfg(feature = "export-stripe")]
pub fn parse_period_hours(
    tsv: &str,
) -> Result<Vec<loams_meter_export::stripe::HourUsage>, ExportError> {
    let bad = |line: &str| ExportError::Invalid(format!("a period hour row: {line:?}"));
    tsv.lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != 3 {
                return Err(bad(line));
            }
            Ok(loams_meter_export::stripe::HourUsage {
                hour_start: f[0].parse().map_err(|_| bad(line))?,
                value: Quantity::parse(f[1]).map_err(|_| bad(line))?,
                close_run: f[2].parse().map_err(|_| bad(line))?,
            })
        })
        .collect()
}

/// The Lago events of `rows`: each window of an exported meter whose
/// subject belongs to a customer, with the customer's subscription.
#[cfg(feature = "export-lago")]
pub fn lago_windows(
    rows: Vec<WindowRow>,
    records: &Records,
    mapping: &ExportMapping,
) -> Vec<loams_meter_export::lago::LagoWindow> {
    let mut by_subject: BTreeMap<&str, &str> = BTreeMap::new();
    for c in &records.customers {
        for s in &c.subjects {
            by_subject.insert(s.as_str(), c.id.as_str());
        }
    }
    rows.into_iter()
        .filter_map(|w| {
            let code = mapping.lago_code(&w.meter)?;
            let customer = by_subject.get(w.subject.as_str())?;
            Some(loams_meter_export::lago::LagoWindow {
                external_subscription_id: mapping.lago_subscription(customer, &w.subject),
                code,
                window: w,
            })
        })
        .collect()
}

/// A usage event read off `_system/_meter` as OpenMeter's exporter takes it.
#[cfg(feature = "export-openmeter")]
pub fn event_row(
    partition: u32,
    offset: u64,
    e: &loams_meter_core::Event,
) -> loams_meter_export::openmeter::EventRow {
    loams_meter_export::openmeter::EventRow {
        subject: e.subject.as_str().to_string(),
        r#type: e.r#type.clone(),
        source: e.source.clone(),
        id: e.id.clone(),
        time: e.time.to_rfc3339(),
        tenantid: e.tenant.as_ref().map(|t| t.to_string()).unwrap_or_default(),
        lease: e.lease.as_ref().map(|l| l.to_string()).unwrap_or_default(),
        data: serde_json::Value::Object(e.data.clone()).to_string(),
        stream_partition: partition,
        stream_offset: offset,
    }
}

/// Events read per partition per OpenMeter request.
pub const OPENMETER_READ: usize = 500;

/// The most reads per partition in one run, so one busy partition does
/// not hold up the others (the next run continues).
pub const OPENMETER_ROUNDS: usize = 20;

/// Stripe's payment state into `SetPaymentState`, as the billing exporter.
#[cfg(feature = "export-stripe")]
#[derive(Debug, Clone)]
pub struct CustomerPayments(pub crate::customers::Customers);

#[cfg(feature = "export-stripe")]
#[async_trait]
impl loams_meter_export::stripe::payment::PaymentStateSink for CustomerPayments {
    async fn set_payment_state(
        &self,
        stripe_customer_id: &str,
        state: loams_meter_export::stripe::payment::PaymentState,
    ) -> Result<(), ExportError> {
        use loams_meter_export::stripe::payment::PaymentState as Stripe;
        let state = match state {
            Stripe::None => crate::store::PaymentState::None,
            Stripe::Card => crate::store::PaymentState::Card,
        };
        self.0
            .set_payment_state(
                &crate::authz::Caller::Service(crate::role::BILLING_SERVICE.into()),
                None,
                Some(stripe_customer_id),
                state,
                "stripe",
            )
            .await
            .map(|_| ())
            .map_err(|e| ExportError::Invalid(format!("SetPaymentState: {e}")))
    }
}

/// `POST /v1/meter/webhooks/stripe`: Stripe's payment-method and customer
/// events (raw body, `Stripe-Signature`). 400 on a bad signature or body,
/// 5xx on anything else (Stripe retries those).
#[cfg(feature = "export-stripe")]
pub fn stripe_webhook_router(
    payments: Arc<loams_meter_export::stripe::payment::StripePayments>,
    clock: Clock,
) -> axum::Router {
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    axum::Router::new()
        .route(
            "/v1/meter/webhooks/stripe",
            axum::routing::post(
                move |axum::extract::State(payments): axum::extract::State<
                    Arc<loams_meter_export::stripe::payment::StripePayments>,
                >,
                      headers: HeaderMap,
                      body: axum::body::Bytes| {
                    let clock = clock.clone();
                    async move {
                        let signature = headers
                            .get("stripe-signature")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("");
                        let now = clock().0.div_euclid(1_000_000_000);
                        match payments.handle_webhook(signature, &body, now).await {
                            Ok(_) => StatusCode::OK.into_response(),
                            Err(ExportError::Webhook(m)) => {
                                (StatusCode::BAD_REQUEST, m).into_response()
                            }
                            Err(e) => {
                                tracing::warn!(%e, "a Stripe webhook was not applied");
                                StatusCode::SERVICE_UNAVAILABLE.into_response()
                            }
                        }
                    }
                },
            ),
        )
        .with_state(payments)
}

/// The exporters the role runs, as configured.
#[derive(Default)]
pub struct Exporters {
    #[cfg(feature = "export-webhook")]
    pub webhook: Option<loams_meter_export::webhook::WebhookExporter>,
    #[cfg(feature = "export-stripe")]
    pub payments: Option<Arc<loams_meter_export::stripe::payment::StripePayments>>,
    /// Stripe's period export (D2403).
    #[cfg(feature = "export-stripe")]
    pub stripe: Option<loams_meter_export::stripe::StripeExporter>,
    #[cfg(feature = "export-lago")]
    pub lago: Option<loams_meter_export::lago::LagoExporter>,
    /// With the cursors it keeps.
    #[cfg(feature = "export-openmeter")]
    pub openmeter: Option<(
        loams_meter_export::openmeter::OpenMeterExporter,
        Arc<dyn ExportCursors>,
    )>,
    /// The provider names (D2403).
    pub mapping: ExportMapping,
}

impl fmt::Debug for Exporters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Exporters").finish_non_exhaustive()
    }
}

/// Runs the configured exporters.
pub struct ExportSchedule<H> {
    pub house: Arc<H>,
    pub store: Arc<dyn crate::store::MeterStore>,
    pub exporters: Exporters,
    pub clock: Clock,
    /// `_system/_meter`, for OpenMeter's raw events.
    pub stream: Option<Arc<dyn crate::owner::PartitionReader>>,
    /// The partitions this node exports from (the counter partitions it
    /// owns, so each is exported by one node); `None`: every partition.
    pub owned: Option<Arc<crate::run::Counters>>,
}

impl<H> fmt::Debug for ExportSchedule<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExportSchedule").finish_non_exhaustive()
    }
}

impl<H: HouseSql> ExportSchedule<H> {
    fn now_secs(&self) -> i64 {
        (self.clock)().0.div_euclid(1_000_000_000)
    }

    /// One run of the window exporters (the webhook and Lago).
    pub async fn windows_once(&self) -> Result<usize, ExportError> {
        let now = self.now_secs();
        let since = now - i64::try_from(LOOKBACK.as_secs()).unwrap_or(0);
        #[allow(unused_mut)]
        let mut sent = 0;
        #[cfg(any(feature = "export-webhook", feature = "export-lago"))]
        {
            #[allow(unused_mut, unused_assignments)]
            let mut wanted = false;
            #[cfg(feature = "export-webhook")]
            {
                wanted |= self.exporters.webhook.is_some();
            }
            #[cfg(feature = "export-lago")]
            {
                wanted |= self.exporters.lago.is_some();
            }
            if wanted {
                let rows = closed_hour_windows(&*self.house, since).await?;
                #[cfg(feature = "export-webhook")]
                if let Some(webhook) = &self.exporters.webhook {
                    sent += webhook.export(&rows, now).await?.sent as usize;
                }
                #[cfg(feature = "export-lago")]
                if let Some(lago) = &self.exporters.lago {
                    let records = self.store.records().await.map_err(ledger_err)?;
                    let windows = lago_windows(rows, &records, &self.exporters.mapping);
                    sent += lago.export(&windows).await?.sent as usize;
                }
            }
        }
        let _ = since;
        Ok(sent)
    }

    /// The partitions this node exports from.
    #[cfg_attr(not(feature = "export-openmeter"), allow(dead_code))]
    async fn partitions(&self) -> Vec<u32> {
        match &self.owned {
            Some(counters) => counters.owned().await.into_iter().map(|(p, _)| p).collect(),
            None => (0..crate::owner::METER_PARTITIONS).collect(),
        }
    }

    /// Whether this node exports `subject` (its partition is owned here).
    #[cfg_attr(not(feature = "export-stripe"), allow(dead_code))]
    fn exports_subject(&self, subject: &str, owned: &[u32]) -> bool {
        self.owned.is_none()
            || owned.contains(&crate::owner::partition_of(
                subject,
                crate::owner::METER_PARTITIONS,
            ))
    }

    /// One run of Stripe's period export (D2403): every period of
    /// [`stripe_periods`] whose subject's partition this node owns. A
    /// customer whose export fails is logged and the others go on; an
    /// off-hour period stops that customer's export (§63 §16).
    pub async fn stripe_once(&self) -> Result<usize, ExportError> {
        #[cfg(feature = "export-stripe")]
        if let Some(stripe) = &self.exporters.stripe {
            let now = self.now_secs();
            let records = self.store.records().await.map_err(ledger_err)?;
            let owned = self.partitions().await;
            let mut sent = 0;
            for p in stripe_periods(&records, &self.exporters.mapping, now) {
                if !self.exports_subject(&p.subject, &owned) {
                    continue;
                }
                let tsv = self
                    .house
                    .query(&period_hours_sql(&p, now))
                    .await
                    .map_err(|e| ExportError::Invalid(format!("House: {e}")))?;
                let export = loams_meter_export::stripe::PeriodExport {
                    event_name: p.event_name.clone(),
                    subject: p.subject.clone(),
                    stripe_customer_id: p.stripe_customer_id.clone(),
                    period_start: p.period.0,
                    included: p.included,
                    hours: parse_period_hours(&tsv)?,
                };
                match stripe.export_period(&export).await {
                    Ok(r) => sent += r.sent as usize,
                    Err(err @ ExportError::OffHourPeriod { .. }) => {
                        tracing::error!(subject = p.subject.as_str(), %err, "Stripe export stopped for this customer");
                    }
                    Err(err) => {
                        tracing::warn!(subject = p.subject.as_str(), event_name = p.event_name.as_str(), %err, "Stripe period export failed; the next run resends");
                    }
                }
            }
            return Ok(sent);
        }
        Ok(0)
    }

    /// One run of OpenMeter's raw-event export (D2403): each owned
    /// partition from the exporter's cursor, [`OPENMETER_READ`] events per
    /// request, at most [`OPENMETER_ROUNDS`] requests per partition.
    pub async fn openmeter_once(&self) -> Result<u64, ExportError> {
        #[cfg(feature = "export-openmeter")]
        if let (Some((exporter, cursors)), Some(stream)) = (&self.exporters.openmeter, &self.stream)
        {
            use loams_meter_export::openmeter::EXPORTER;
            let mut sent = 0;
            for partition in self.partitions().await {
                for _ in 0..OPENMETER_ROUNDS {
                    let from = cursors.get(EXPORTER, partition).await?.unwrap_or(0);
                    let events = stream
                        .read(partition, from, OPENMETER_READ)
                        .await
                        .map_err(|e| ExportError::Invalid(format!("meter stream: {e}")))?;
                    let Some(last) = events.last().map(|(o, _)| *o) else {
                        break;
                    };
                    let rows: Vec<_> = events
                        .iter()
                        .map(|(o, e)| event_row(partition, *o, e))
                        .collect();
                    sent += exporter.export(&rows).await?.sent;
                    // Every row read was exported or filtered: the cursor
                    // is past the last one (records the reader skipped as
                    // malformed included).
                    if cursors.get(EXPORTER, partition).await?.unwrap_or(0) <= last {
                        cursors.put(EXPORTER, partition, last + 1).await?;
                    }
                    if events.len() < OPENMETER_READ {
                        break;
                    }
                }
            }
            return Ok(sent);
        }
        Ok(0)
    }

    /// One Stripe poll over every customer with a Stripe id.
    pub async fn poll_once(&self) -> Result<usize, ExportError> {
        #[cfg(feature = "export-stripe")]
        if let Some(payments) = &self.exporters.payments {
            let ids: Vec<String> = self
                .store
                .records()
                .await
                .map_err(ledger_err)?
                .customers
                .into_iter()
                .filter_map(|c| c.stripe_customer_id)
                .collect();
            let report = payments.poll(&ids).await;
            for (id, err) in &report.failed {
                tracing::warn!(customer = id.as_str(), %err, "Stripe payment state not read");
            }
            return Ok(ids.len() - report.failed.len());
        }
        Ok(0)
    }

    /// Runs until `stop` is notified.
    pub async fn run(self, stop: Arc<Notify>) {
        let mut next_poll = self.now_secs();
        loop {
            if let Err(err) = self.windows_once().await {
                tracing::warn!(%err, "the meter's window export failed; the next run resends");
            }
            if let Err(err) = self.stripe_once().await {
                tracing::warn!(%err, "the meter's Stripe period export failed; the next run resends");
            }
            if let Err(err) = self.openmeter_once().await {
                tracing::warn!(%err, "the meter's OpenMeter export failed; the next run resends");
            }
            if self.now_secs() >= next_poll {
                next_poll = self.now_secs() + i64::try_from(POLL_EVERY.as_secs()).unwrap_or(0);
                if let Err(err) = self.poll_once().await {
                    tracing::warn!(%err, "the Stripe payment-state poll failed");
                }
            }
            tokio::select! {
                () = stop.notified() => return,
                () = tokio::time::sleep(EXPORT_EVERY) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hour_windows_parse_back() {
        let tsv = "house_cpu_ms\torg_a\t1792400400\t{\"region\":\"eu\"}\t12.500000000\t3\t17\n\
                   house_cpu_ms\torg_b\t1792400400\t\t1\t1\t17\n";
        let rows = parse_hour_windows(tsv).expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].subject, "org_a");
        assert_eq!(rows[0].value, Quantity::parse("12.5").expect("q"));
        assert_eq!(rows[0].group(), "{\"region\":\"eu\"}");
        assert_eq!(rows[1].group(), "");
        assert_eq!(rows[1].close_run, 17);
        assert!(parse_hour_windows("a\tb\n").is_err());
        assert!(hour_windows_sql(0).contains("_meter.windows FINAL"));
    }

    #[tokio::test]
    async fn the_ledger_and_cursors_survive_in_the_document() {
        let doc = Arc::new(crate::store::MemDoc::default());
        let clock: Clock = Arc::new(|| loams_meter_core::UtcNanos(1_792_400_400_000_000_000));
        let exports = DocExports::new(doc.clone(), clock.clone());
        let key = LedgerKey {
            exporter: "webhook".into(),
            meter: "house_cpu_ms".into(),
            subject: "org_a".into(),
            window_start: 1_792_400_400,
        };
        let entry = LedgerEntry {
            identifier: "id-1".into(),
            value: Quantity::parse("2").expect("q"),
            total: Quantity::parse("2").expect("q"),
            revision: 7,
            seq: 0,
            sent: false,
        };
        ExportLedger::put(&exports, &key, &entry)
            .await
            .expect("put");
        ExportCursors::put(&exports, "openmeter", 3, 42)
            .await
            .expect("cursor");
        let again = DocExports::new(doc, clock);
        assert_eq!(
            ExportLedger::get(&again, &key).await.expect("get"),
            Some(entry)
        );
        assert_eq!(
            ExportCursors::get(&again, "openmeter", 3)
                .await
                .expect("get"),
            Some(42)
        );
        // A window older than the retention is dropped on the next write.
        let old = LedgerKey {
            window_start: 0,
            ..key.clone()
        };
        let e = ExportLedger::get(&again, &key)
            .await
            .expect("get")
            .expect("there");
        ExportLedger::put(&again, &old, &e).await.expect("put");
        assert_eq!(ExportLedger::get(&again, &old).await.expect("get"), None);
    }
}
