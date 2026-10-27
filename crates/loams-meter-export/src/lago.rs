//! The Lago exporter (§63 §16; plan ME1 Task 26; D2214).
//!
//! **Over HTTP only.** Lago is AGPL-3.0: Loams never links, ships or copies
//! it (D11); this module speaks its public REST API.
//!
//! It sends closed windows as Lago events to `POST /api/v1/events/batch`
//! (at most 100 a request): `transaction_id`, `external_subscription_id`,
//! `code` (the billable metric), `timestamp` (the window's start, Unix
//! seconds) and `properties` (the value under `value_property`, plus the
//! window's group-by values). The billable metric sums that property.
//!
//! **Re-closed windows (D2214).** Lago's events are additive and Lago keeps
//! the first event of a `transaction_id`, so a re-closed window cannot
//! overwrite the earlier event. The exporter sends the **difference**
//! between the window's value and the total Lago accepted for it so far,
//! under a new `transaction_id` (`loams:<code>:<subject>:<window start>:
//! <close_run>`, with `:g<hash>` for a group and `-<n>` when the same close
//! run must send again). The difference can be negative (a re-close that
//! lowers a window); Lago's `sum_agg` adds it. The [`ExportLedger`] holds
//! each planned event before the request; a run that finds one unaccepted
//! resends it unchanged first and computes a new difference only on the next
//! run, so the window's total in Lago is always the sum of distinct events.
//! A Lago that refuses a known `transaction_id` (422 `value_already_exist`)
//! has that event already: the batch is then sent one event at a time and
//! the duplicate counts as accepted.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::ledger::{ExportLedger, LedgerEntry, LedgerKey};
use crate::retry::{self, RetryPolicy};
use crate::{ExportError, Quantity, WindowRow};

/// The ledger's exporter name.
pub const EXPORTER: &str = "lago";
/// Lago's batch limit.
pub const MAX_BATCH: usize = 100;
const MAX_ID: usize = 100;

/// The exporter's settings.
#[derive(Clone)]
pub struct LagoConfig {
    /// Lago's API base URL, for example `https://api.getlago.com` or a
    /// self-hosted `http://lago-api:3000`.
    pub endpoint: String,
    pub api_key: String,
    /// The property the billable metric sums.
    pub value_property: String,
    /// Events per request, at most [`MAX_BATCH`].
    pub batch_size: usize,
    pub retry: RetryPolicy,
    pub timeout: Duration,
}

impl LagoConfig {
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>) -> LagoConfig {
        LagoConfig {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            value_property: "value".into(),
            batch_size: MAX_BATCH,
            retry: RetryPolicy::default(),
            timeout: Duration::from_secs(30),
        }
    }
}

impl fmt::Debug for LagoConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LagoConfig")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"<redacted>")
            .field("value_property", &self.value_property)
            .field("batch_size", &self.batch_size)
            .field("retry", &self.retry)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// One closed window to export, with its Lago names.
#[derive(Debug, Clone)]
pub struct LagoWindow {
    pub window: WindowRow,
    /// The customer's Lago subscription.
    pub external_subscription_id: String,
    /// The billable metric's code (by convention the meter slug).
    pub code: String,
}

/// The `transaction_id` of one send (module docs).
pub fn transaction_id(code: &str, w: &WindowRow, seq: u32) -> String {
    let mut id = format!(
        "loams:{code}:{}:{}:{}",
        w.subject, w.window_start, w.close_run
    );
    if !w.group().is_empty() {
        let h = hex::encode(Sha256::digest(w.group().as_bytes()));
        id.push_str(&format!(":g{}", &h[..16]));
    }
    if seq > 0 {
        id.push_str(&format!("-{seq}"));
    }
    if id.len() > MAX_ID {
        id = format!("loams:h:{}", hex::encode(Sha256::digest(id.as_bytes())));
    }
    id
}

/// What one export run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportReport {
    /// Events Lago accepted.
    pub sent: u32,
    /// Of those, differences for windows sent before (re-closes).
    pub adjustments: u32,
    /// Windows whose total in Lago is already their value.
    pub unchanged: u32,
    /// Unaccepted events from an earlier run, sent again unchanged.
    pub resent: u32,
}

/// Sends closed windows to Lago.
#[derive(Debug, Clone)]
pub struct LagoExporter {
    http: reqwest::Client,
    cfg: LagoConfig,
    ledger: Arc<dyn ExportLedger>,
}

struct Planned {
    key: LedgerKey,
    entry: LedgerEntry,
    event: Value,
}

impl LagoExporter {
    pub fn new(
        cfg: LagoConfig,
        ledger: Arc<dyn ExportLedger>,
    ) -> Result<LagoExporter, ExportError> {
        if !(cfg.endpoint.starts_with("https://") || cfg.endpoint.starts_with("http://")) {
            return Err(ExportError::Invalid(format!(
                "lago: endpoint {:?} is not an http(s) URL",
                cfg.endpoint
            )));
        }
        if cfg.api_key.trim().is_empty() {
            return Err(ExportError::Invalid("lago: no API key".into()));
        }
        if cfg.batch_size == 0 || cfg.batch_size > MAX_BATCH {
            return Err(ExportError::Invalid(format!(
                "lago: batch_size must be 1..={MAX_BATCH}"
            )));
        }
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(|e| ExportError::Invalid(format!("lago: HTTP client: {e}")))?;
        Ok(LagoExporter { http, cfg, ledger })
    }

    /// Exports `windows`. Stops at the first error; planned events stay in
    /// the ledger and are resent by the next run.
    pub async fn export(&self, windows: &[LagoWindow]) -> Result<ExportReport, ExportError> {
        let mut report = ExportReport::default();
        let mut planned: Vec<Planned> = Vec::new();
        for lw in windows {
            let w = &lw.window;
            if lw.external_subscription_id.is_empty() || lw.code.is_empty() {
                return Err(ExportError::Invalid(format!(
                    "lago: window {} of {} has no subscription or metric code",
                    w.window_start, w.subject
                )));
            }
            let key = LedgerKey {
                exporter: EXPORTER.to_owned(),
                meter: lw.code.clone(),
                subject: if w.group().is_empty() {
                    w.subject.clone()
                } else {
                    format!("{}|{}", w.subject, w.group())
                },
                window_start: w.window_start,
            };
            let prev = self.ledger.get(&key).await?;
            if let Some(e) = prev.as_ref().filter(|e| !e.sent) {
                // Unaccepted from an earlier run: the same event again,
                // before any new difference.
                report.resent += 1;
                planned.push(Planned {
                    event: self.event(lw, &e.identifier, e.value)?,
                    key,
                    entry: e.clone(),
                });
                continue;
            }
            let total = prev.as_ref().map_or(Quantity::ZERO, |e| e.total);
            let delta = w.value.saturating_sub(total);
            if delta.is_zero() {
                report.unchanged += 1;
                continue;
            }
            let seq = match &prev {
                Some(e) if e.revision == w.close_run => e.seq + 1,
                _ => 0,
            };
            if prev.is_some() {
                report.adjustments += 1;
            }
            let entry = LedgerEntry {
                identifier: transaction_id(&lw.code, w, seq),
                value: delta,
                total: w.value,
                revision: w.close_run,
                seq,
                sent: false,
            };
            self.ledger.put(&key, &entry).await?;
            planned.push(Planned {
                event: self.event(lw, &entry.identifier, delta)?,
                key,
                entry,
            });
        }
        for chunk in planned.chunks(self.cfg.batch_size) {
            let events: Vec<&Value> = chunk.iter().map(|p| &p.event).collect();
            match self.send(&events).await {
                Ok(()) => {}
                // A Lago that checks `transaction_id` at ingest refuses a
                // batch holding one it already has (an event accepted
                // before a timeout). Send that batch one event at a time;
                // a duplicate alone is the accepted event.
                Err(e) if is_duplicate(&e) => {
                    for event in &events {
                        match self.send(&[*event]).await {
                            Ok(()) => {}
                            Err(e) if is_duplicate(&e) => {}
                            Err(e) => return Err(e),
                        }
                    }
                }
                Err(e) => return Err(e),
            }
            for p in chunk {
                let done = LedgerEntry {
                    sent: true,
                    ..p.entry.clone()
                };
                self.ledger.put(&p.key, &done).await?;
            }
            report.sent += chunk.len() as u32;
        }
        Ok(report)
    }

    async fn send(&self, events: &[&Value]) -> Result<(), ExportError> {
        let body = serde_json::to_vec(&json!({ "events": events }))
            .map_err(|e| ExportError::Invalid(format!("lago: {e}")))?;
        let url = format!(
            "{}/api/v1/events/batch",
            self.cfg.endpoint.trim_end_matches('/')
        );
        retry::send(EXPORTER, &self.cfg.retry, lago_message, || {
            self.http
                .post(&url)
                .bearer_auth(&self.cfg.api_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone())
                .send()
        })
        .await?;
        Ok(())
    }

    fn event(&self, lw: &LagoWindow, id: &str, value: Quantity) -> Result<Value, ExportError> {
        let mut props = Map::new();
        if !lw.window.group().is_empty() {
            let group: Map<String, Value> =
                serde_json::from_str(lw.window.group()).map_err(|e| {
                    ExportError::Invalid(format!(
                        "lago: group key of {} is not a JSON object: {e}",
                        lw.window.subject
                    ))
                })?;
            props.extend(group);
        }
        props.insert(self.cfg.value_property.clone(), value.to_string().into());
        Ok(json!({
            "transaction_id": id,
            "external_subscription_id": lw.external_subscription_id,
            "code": lw.code,
            "timestamp": lw.window.window_start,
            "properties": props,
        }))
    }
}

/// Lago's refusal of a `transaction_id` it already holds.
fn is_duplicate(e: &ExportError) -> bool {
    matches!(e, ExportError::Rejected { status: 422, message, .. } if message.contains("value_already_exist"))
}

/// Lago's error body (`{"status", "error", "code", "error_details"}`).
fn lago_message(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => {
            let part = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
            let (error, code) = (part("error"), part("code"));
            let details = v
                .get("error_details")
                .map(Value::to_string)
                .unwrap_or_default();
            [error, code, details]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(": ")
        }
        Err(_) => "no Lago error body".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WindowSize;

    fn w(group: &str, run: u64) -> WindowRow {
        WindowRow {
            meter: "llm_output_tokens".into(),
            subject: "org_42".into(),
            window_size: WindowSize::Hour,
            window_start: 1_800_000_000,
            group_key: group.into(),
            value: Quantity::from_units(5),
            events: 2,
            close_run: run,
        }
    }

    #[test]
    fn transaction_ids() {
        assert_eq!(
            transaction_id("tokens", &w("{}", 3), 0),
            "loams:tokens:org_42:1800000000:3"
        );
        assert_eq!(
            transaction_id("tokens", &w("", 3), 1),
            "loams:tokens:org_42:1800000000:3-1"
        );
        let a = transaction_id("tokens", &w(r#"{"model":"a"}"#, 3), 0);
        let b = transaction_id("tokens", &w(r#"{"model":"b"}"#, 3), 0);
        assert!(a.contains(":g") && a != b, "{a} {b}");
        let mut long = w("", 1);
        long.subject = "s".repeat(120);
        assert!(transaction_id("tokens", &long, 0).len() <= MAX_ID);
    }

    #[test]
    fn lago_message_reads_the_error_body() {
        assert_eq!(
            lago_message(
                r#"{"status":422,"error":"Unprocessable Entity","code":"validation_errors","error_details":{"transaction_id":["value_already_exist"]}}"#
            ),
            r#"Unprocessable Entity: validation_errors: {"transaction_id":["value_already_exist"]}"#
        );
    }

    #[test]
    fn config_debug_hides_the_key() {
        assert!(
            !format!("{:?}", LagoConfig::new("http://x", "lago_secret")).contains("lago_secret")
        );
    }
}
