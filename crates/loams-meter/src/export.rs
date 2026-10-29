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
//! Not wired yet: Stripe's period export (it needs each customer's
//! included amount and the Stripe meter's event name per feature), Lago
//! (it needs the subscription and billable-metric code per customer and
//! meter) and OpenMeter's raw-event export (it needs a stream reader with
//! the events' offsets). The ledger and cursors they take are here.

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
use crate::store::VersionedDoc;

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

    /// One run of the window exporters.
    pub async fn windows_once(&self) -> Result<usize, ExportError> {
        let now = self.now_secs();
        let since = now - i64::try_from(LOOKBACK.as_secs()).unwrap_or(0);
        #[cfg(feature = "export-webhook")]
        if let Some(webhook) = &self.exporters.webhook {
            let rows = closed_hour_windows(&*self.house, since).await?;
            let report = webhook.export(&rows, now).await?;
            return Ok(report.sent as usize);
        }
        let _ = since;
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
