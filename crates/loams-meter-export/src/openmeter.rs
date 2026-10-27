//! The OpenMeter exporter (§63 §16; plan ME1 Task 25; D2213).
//!
//! It sends raw usage events, filtered by type, **unchanged but for the
//! `loamslease` extension** (the lease that admitted the work is internal to
//! Loams' enforcement), to OpenMeter's `POST /api/v1/events` as
//! `application/cloudevents-batch+json`. For operators who prefer OpenMeter's
//! billing and catalogue (or Kong Konnect).
//!
//! **Exactly once, as far as OpenMeter allows.** OpenMeter dedupes by
//! `source` + `id`, which a retry keeps, but its dedup window is
//! configuration (off in its example config). So the exporter also keeps its
//! own cursor per `_system/_meter` partition (the next offset to export) in
//! [`ExportCursors`] and moves it only after OpenMeter accepted the batch.
//! A crash between the accept and the cursor write resends that batch with
//! the same ids, which OpenMeter's dedup drops when it is on.
//!
//! Rows come from `_meter.events FINAL` in partition and offset order; rows
//! whose type is not exported still move the cursor.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::ExportError;
use crate::ledger::ExportCursors;
use crate::retry::{self, RetryPolicy};

/// The cursor's exporter name.
pub const EXPORTER: &str = "openmeter";
/// The CloudEvents extension this exporter strips.
pub const LEASE_EXTENSION: &str = "loamslease";
/// The batch content type.
pub const BATCH_CONTENT_TYPE: &str = "application/cloudevents-batch+json";

/// The exporter's settings.
#[derive(Clone)]
pub struct OpenMeterConfig {
    /// OpenMeter's base URL, for example `https://openmeter.cloud` or a
    /// self-hosted `http://openmeter:8888`.
    pub endpoint: String,
    /// A bearer token (OpenMeter Cloud, Kong Konnect); none for a
    /// self-hosted OpenMeter without auth.
    pub token: Option<String>,
    /// Type prefixes to export; empty exports every Loams usage type
    /// (`io.loams.dev.usage.`).
    pub types: Vec<String>,
    /// Events per request.
    pub batch_size: usize,
    pub retry: RetryPolicy,
    pub timeout: Duration,
}

impl OpenMeterConfig {
    pub fn new(endpoint: impl Into<String>) -> OpenMeterConfig {
        OpenMeterConfig {
            endpoint: endpoint.into(),
            token: None,
            types: Vec::new(),
            batch_size: 500,
            retry: RetryPolicy::default(),
            timeout: Duration::from_secs(30),
        }
    }

    fn exports(&self, ty: &str) -> bool {
        if self.types.is_empty() {
            ty.starts_with("io.loams.dev.usage.")
        } else {
            self.types.iter().any(|p| ty.starts_with(p.as_str()))
        }
    }
}

impl fmt::Debug for OpenMeterConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenMeterConfig")
            .field("endpoint", &self.endpoint)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("types", &self.types)
            .field("batch_size", &self.batch_size)
            .field("retry", &self.retry)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// One row of `_meter.events` (§63 §7.5) with its stream position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub subject: String,
    pub r#type: String,
    pub source: String,
    pub id: String,
    /// RFC 3339 with nanoseconds, as the producer sent it.
    pub time: String,
    /// `<org>/<namespace>`, or empty.
    pub tenantid: String,
    /// `loamslease`, empty when absent; never exported.
    pub lease: String,
    /// The event's `data`, JSON text (an object).
    pub data: String,
    pub stream_partition: u32,
    pub stream_offset: u64,
}

/// The CloudEvent OpenMeter receives for a row: the Loams usage profile
/// without `loamslease`.
pub fn to_openmeter(row: &EventRow) -> Result<Value, ExportError> {
    let data: Value = serde_json::from_str(&row.data).map_err(|e| {
        ExportError::Invalid(format!(
            "openmeter: event {} from {}: data is not JSON: {e}",
            row.id, row.source
        ))
    })?;
    let mut ce = Map::new();
    ce.insert("specversion".into(), "1.0".into());
    ce.insert("id".into(), row.id.clone().into());
    ce.insert("source".into(), row.source.clone().into());
    ce.insert("type".into(), row.r#type.clone().into());
    ce.insert("subject".into(), row.subject.clone().into());
    ce.insert("time".into(), row.time.clone().into());
    ce.insert("datacontenttype".into(), "application/json".into());
    if row.r#type.starts_with("io.loams.dev.") {
        ce.insert(
            "dataschema".into(),
            format!("urn:loams:json:{}", row.r#type).into(),
        );
    }
    if !row.tenantid.is_empty() {
        ce.insert("tenantid".into(), row.tenantid.clone().into());
    }
    ce.insert("data".into(), data);
    Ok(Value::Object(ce))
}

/// Removes `loamslease` from a CloudEvent in JSON form, for callers that
/// hold events rather than rows.
pub fn strip_lease(mut ce: Value) -> Value {
    if let Value::Object(map) = &mut ce {
        map.remove(LEASE_EXTENSION);
    }
    ce
}

/// What one export call did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportReport {
    /// Events OpenMeter accepted.
    pub sent: u64,
    /// Rows before a partition's cursor (exported already).
    pub skipped: u64,
    /// Rows whose type is not exported (the cursor moved past them).
    pub filtered: u64,
    /// Requests OpenMeter accepted.
    pub batches: u32,
}

/// Sends raw usage events to OpenMeter.
#[derive(Debug, Clone)]
pub struct OpenMeterExporter {
    http: reqwest::Client,
    cfg: OpenMeterConfig,
    cursors: Arc<dyn ExportCursors>,
}

impl OpenMeterExporter {
    pub fn new(
        cfg: OpenMeterConfig,
        cursors: Arc<dyn ExportCursors>,
    ) -> Result<OpenMeterExporter, ExportError> {
        if !(cfg.endpoint.starts_with("https://") || cfg.endpoint.starts_with("http://")) {
            return Err(ExportError::Invalid(format!(
                "openmeter: endpoint {:?} is not an http(s) URL",
                cfg.endpoint
            )));
        }
        if cfg.batch_size == 0 {
            return Err(ExportError::Invalid("openmeter: batch_size is 0".into()));
        }
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(|e| ExportError::Invalid(format!("openmeter: HTTP client: {e}")))?;
        Ok(OpenMeterExporter { http, cfg, cursors })
    }

    /// Exports `rows` (any order; rows of several partitions allowed) past
    /// each partition's cursor. Stops at the first error: the cursor then
    /// points at the first batch OpenMeter did not accept.
    pub async fn export(&self, rows: &[EventRow]) -> Result<ExportReport, ExportError> {
        let mut sorted: Vec<&EventRow> = rows.iter().collect();
        sorted.sort_by_key(|r| (r.stream_partition, r.stream_offset));
        let mut report = ExportReport::default();
        let mut i = 0;
        while i < sorted.len() {
            let partition = sorted[i].stream_partition;
            let end = sorted[i..]
                .iter()
                .position(|r| r.stream_partition != partition)
                .map_or(sorted.len(), |n| i + n);
            self.export_partition(partition, &sorted[i..end], &mut report)
                .await?;
            i = end;
        }
        Ok(report)
    }

    async fn export_partition(
        &self,
        partition: u32,
        rows: &[&EventRow],
        report: &mut ExportReport,
    ) -> Result<(), ExportError> {
        let cursor = self.cursors.get(EXPORTER, partition).await?.unwrap_or(0);
        let mut pending: Vec<Value> = Vec::new();
        // The offset after the last row covered by `pending` (sent or not).
        let mut covered: Option<u64> = None;
        for row in rows {
            if row.stream_offset < cursor {
                report.skipped += 1;
                continue;
            }
            if self.cfg.exports(&row.r#type) {
                pending.push(to_openmeter(row)?);
            } else {
                report.filtered += 1;
            }
            covered = Some(row.stream_offset + 1);
            if pending.len() >= self.cfg.batch_size {
                self.flush(partition, &mut pending, covered, report).await?;
            }
        }
        self.flush(partition, &mut pending, covered, report).await
    }

    /// Sends `pending` (if any), then moves the cursor to `covered`.
    async fn flush(
        &self,
        partition: u32,
        pending: &mut Vec<Value>,
        covered: Option<u64>,
        report: &mut ExportReport,
    ) -> Result<(), ExportError> {
        if !pending.is_empty() {
            let body = serde_json::to_vec(&*pending)
                .map_err(|e| ExportError::Invalid(format!("openmeter: {e}")))?;
            let url = format!("{}/api/v1/events", self.cfg.endpoint.trim_end_matches('/'));
            retry::send(EXPORTER, &self.cfg.retry, openmeter_message, || {
                let mut req = self
                    .http
                    .post(&url)
                    .header(reqwest::header::CONTENT_TYPE, BATCH_CONTENT_TYPE)
                    .body(body.clone());
                if let Some(t) = &self.cfg.token {
                    req = req.bearer_auth(t);
                }
                req.send()
            })
            .await?;
            report.sent += pending.len() as u64;
            report.batches += 1;
            pending.clear();
        }
        if let Some(next) = covered {
            self.cursors.put(EXPORTER, partition, next).await?;
        }
        Ok(())
    }
}

/// OpenMeter's problem details (`title`, `detail`), or the status alone.
fn openmeter_message(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => {
            let title = v.get("title").and_then(Value::as_str).unwrap_or("");
            let detail = v.get("detail").and_then(Value::as_str).unwrap_or("");
            match (title.is_empty(), detail.is_empty()) {
                (false, false) => format!("{title}: {detail}"),
                (false, true) => title.to_owned(),
                (true, false) => detail.to_owned(),
                (true, true) => "no message".into(),
            }
        }
        Err(_) => "no problem details".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> EventRow {
        EventRow {
            subject: "org_42".into(),
            r#type: "io.loams.dev.usage.house.query.v1".into(),
            source: "loams://prod-eu1/house/node-3".into(),
            id: "q-7f3a9c/1".into(),
            time: "2026-10-19T09:14:03.218734Z".into(),
            tenantid: "42/7".into(),
            lease: "l9k2m4q8x1".into(),
            data: r#"{"namespace":"7","cpu_ms":1840}"#.into(),
            stream_partition: 3,
            stream_offset: 10,
        }
    }

    #[test]
    fn row_becomes_the_usage_event_without_the_lease() {
        let ce = to_openmeter(&row()).expect("event");
        assert_eq!(ce["specversion"], "1.0");
        assert_eq!(ce["subject"], "org_42");
        assert_eq!(ce["datacontenttype"], "application/json");
        assert_eq!(
            ce["dataschema"],
            "urn:loams:json:io.loams.dev.usage.house.query.v1"
        );
        assert_eq!(ce["tenantid"], "42/7");
        assert_eq!(ce["data"]["cpu_ms"], 1840);
        assert!(ce.get(LEASE_EXTENSION).is_none());
        assert!(!ce.to_string().contains("l9k2m4q8x1"));
    }

    #[test]
    fn strip_lease_removes_only_the_lease() {
        let ce = serde_json::json!({"id": "1", "loamslease": "abc", "tenantid": "1/2"});
        assert_eq!(
            strip_lease(ce),
            serde_json::json!({"id": "1", "tenantid": "1/2"})
        );
    }

    #[test]
    fn bad_data_is_refused() {
        let mut r = row();
        r.data = "{not json".into();
        assert!(to_openmeter(&r).is_err());
    }

    #[test]
    fn type_filter() {
        let mut c = OpenMeterConfig::new("http://x");
        assert!(c.exports("io.loams.dev.usage.llm.tokens.v1"));
        assert!(!c.exports("io.loams.dev.meter.threshold.v1"));
        c.types = vec!["io.loams.dev.usage.llm.".into()];
        assert!(c.exports("io.loams.dev.usage.llm.tokens.v1"));
        assert!(!c.exports("io.loams.dev.usage.house.query.v1"));
    }

    #[test]
    fn config_debug_hides_the_token() {
        let mut c = OpenMeterConfig::new("http://x");
        c.token = Some("om_secret".into());
        assert!(!format!("{c:?}").contains("om_secret"));
    }
}
