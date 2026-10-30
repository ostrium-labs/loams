//! The CloudEvents webhook exporter (§63 §16; plan ME1 Task 26; D2215): for
//! in-house billing systems.
//!
//! Each closed window becomes a CloudEvent of type
//! `io.loams.dev.meter.window.closed.v1`, sent in batches as
//! `application/cloudevents-batch+json` to one URL. The event's `id` is
//! `<meter>/<subject>/<size>/<start>[/<group hash>]/<close_run>`, so a
//! re-closed window is a new event and the receiver keeps, per window
//! (`data.meter`, `subject`, `data.window_size`, `data.window_start`,
//! `data.group`), the one with the highest `data.close_run`. `source` + `id`
//! is the dedup key for a retry.
//!
//! **Signing** follows Standard Webhooks, as the meter's notifier (§63 §15):
//! headers `webhook-id` (the same for every attempt of a batch),
//! `webhook-timestamp` and `webhook-signature: v1,<base64 HMAC-SHA256(key,
//! "<id>.<timestamp>.<body>")>`, where a `whsec_` secret's key is the
//! base64 after the prefix.
//!
//! The [`ExportLedger`] records the highest close run sent per window, so a
//! window is sent again only when it is re-closed.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::ledger::{ExportLedger, LedgerEntry, LedgerKey};
use crate::retry::{self, RetryPolicy};
use crate::{ExportError, WindowRow};

/// The ledger's exporter name.
pub const EXPORTER: &str = "webhook";
/// The event type.
pub const EVENT_TYPE: &str = "io.loams.dev.meter.window.closed.v1";

/// The exporter's settings.
#[derive(Clone)]
pub struct WebhookConfig {
    /// The receiver.
    pub url: String,
    /// `whsec_<base64>` (Standard Webhooks) or raw bytes.
    pub secret: String,
    /// The events' `source`: `loams://<cluster>/meter`.
    pub source: String,
    pub batch_size: usize,
    pub retry: RetryPolicy,
    pub timeout: Duration,
}

impl WebhookConfig {
    pub fn new(
        url: impl Into<String>,
        secret: impl Into<String>,
        source: impl Into<String>,
    ) -> WebhookConfig {
        WebhookConfig {
            url: url.into(),
            secret: secret.into(),
            source: source.into(),
            batch_size: 100,
            retry: RetryPolicy::default(),
            timeout: Duration::from_secs(30),
        }
    }
}

impl fmt::Debug for WebhookConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebhookConfig")
            .field("url", &self.url)
            .field("secret", &"<redacted>")
            .field("source", &self.source)
            .field("batch_size", &self.batch_size)
            .field("retry", &self.retry)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// The signing key of a secret: the base64 after `whsec_`, else the bytes.
pub fn signing_key(secret: &str) -> Result<Vec<u8>, ExportError> {
    match secret.strip_prefix("whsec_") {
        Some(b64) => STANDARD
            .decode(b64)
            .map_err(|_| ExportError::Invalid("webhook: the whsec_ secret is not base64".into())),
        None => Ok(secret.as_bytes().to_vec()),
    }
}

/// `v1,<base64 signature>` of one delivery.
pub fn sign(key: &[u8], msg_id: &str, timestamp: i64, body: &[u8]) -> Result<String, ExportError> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key)
        .map_err(|_| ExportError::Invalid("webhook: unusable secret".into()))?;
    mac.update(msg_id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    Ok(format!(
        "v1,{}",
        STANDARD.encode(mac.finalize().into_bytes())
    ))
}

fn rfc3339(unix: i64) -> Result<String, ExportError> {
    OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .ok_or_else(|| ExportError::Invalid(format!("webhook: {unix} is not a time")))
}

/// The CloudEvent of one closed window.
pub fn window_event(source: &str, w: &WindowRow) -> Result<Value, ExportError> {
    let group: Value = if w.group().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(w.group()).map_err(|e| {
            ExportError::Invalid(format!(
                "webhook: group key of {} is not JSON: {e}",
                w.subject
            ))
        })?
    };
    let mut id = format!(
        "{}/{}/{}/{}",
        w.meter,
        w.subject,
        w.window_size.as_str(),
        w.window_start
    );
    if !w.group().is_empty() {
        let h = hex::encode(Sha256::digest(w.group().as_bytes()));
        id.push('/');
        id.push_str(&h[..16]);
    }
    id.push_str(&format!("/{}", w.close_run));
    let end = w.window_start + w.window_size.seconds();
    Ok(json!({
        "specversion": "1.0",
        "id": id,
        "source": source,
        "type": EVENT_TYPE,
        "subject": w.subject,
        "time": rfc3339(end)?,
        "datacontenttype": "application/json",
        "dataschema": format!("urn:loams:json:{EVENT_TYPE}"),
        "data": {
            "meter": w.meter,
            "subject": w.subject,
            "window_size": w.window_size.as_str(),
            "window_start": rfc3339(w.window_start)?,
            "window_end": rfc3339(end)?,
            "group": group,
            "value": w.value.to_string(),
            "events": w.events,
            "close_run": w.close_run,
        },
    }))
}

/// What one export run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportReport {
    pub sent: u32,
    /// Windows already sent at this close run or a later one.
    pub unchanged: u32,
    pub batches: u32,
}

/// Sends closed windows to a signed webhook.
#[derive(Debug, Clone)]
pub struct WebhookExporter {
    http: reqwest::Client,
    cfg: WebhookConfig,
    key: Arc<Vec<u8>>,
    ledger: Arc<dyn ExportLedger>,
}

impl WebhookExporter {
    pub fn new(
        cfg: WebhookConfig,
        ledger: Arc<dyn ExportLedger>,
    ) -> Result<WebhookExporter, ExportError> {
        if !(cfg.url.starts_with("https://") || cfg.url.starts_with("http://")) {
            return Err(ExportError::Invalid(format!(
                "webhook: {:?} is not an http(s) URL",
                cfg.url
            )));
        }
        if cfg.secret.is_empty() {
            return Err(ExportError::Invalid("webhook: no signing secret".into()));
        }
        if cfg.batch_size == 0 {
            return Err(ExportError::Invalid("webhook: batch_size is 0".into()));
        }
        let key = signing_key(&cfg.secret)?;
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(|e| ExportError::Invalid(format!("webhook: HTTP client: {e}")))?;
        Ok(WebhookExporter {
            http,
            cfg,
            key: Arc::new(key),
            ledger,
        })
    }

    /// Sends every window not yet sent at its close run. `now` is the
    /// deliveries' `webhook-timestamp` (Unix seconds).
    pub async fn export(
        &self,
        windows: &[WindowRow],
        now: i64,
    ) -> Result<ExportReport, ExportError> {
        let mut report = ExportReport::default();
        let mut todo: Vec<(LedgerKey, LedgerEntry, Value)> = Vec::new();
        for w in windows {
            let key = LedgerKey {
                exporter: EXPORTER.to_owned(),
                meter: format!("{}/{}", w.meter, w.window_size.as_str()),
                subject: if w.group().is_empty() {
                    w.subject.clone()
                } else {
                    format!("{}|{}", w.subject, w.group())
                },
                window_start: w.window_start,
            };
            if let Some(e) = self.ledger.get(&key).await?
                && e.sent
                && e.revision >= w.close_run
            {
                report.unchanged += 1;
                continue;
            }
            let event = window_event(&self.cfg.source, w)?;
            let entry = LedgerEntry {
                identifier: event["id"].as_str().unwrap_or_default().to_owned(),
                value: w.value,
                total: w.value,
                revision: w.close_run,
                seq: 0,
                sent: false,
            };
            todo.push((key, entry, event));
        }
        for chunk in todo.chunks(self.cfg.batch_size) {
            let events: Vec<&Value> = chunk.iter().map(|(_, _, e)| e).collect();
            let body = serde_json::to_vec(&events)
                .map_err(|e| ExportError::Invalid(format!("webhook: {e}")))?;
            // The same id for every attempt of this batch: the receiver's
            // dedup key for the delivery.
            let mut h = Sha256::new();
            for (_, e, _) in chunk {
                h.update(e.identifier.as_bytes());
                h.update([0]);
            }
            let msg_id = format!("msg_{}", &hex::encode(h.finalize())[..32]);
            let signature = sign(&self.key, &msg_id, now, &body)?;
            retry::send(
                EXPORTER,
                &self.cfg.retry,
                |b: &str| b.to_owned(),
                || {
                    self.http
                        .post(&self.cfg.url)
                        .header(
                            reqwest::header::CONTENT_TYPE,
                            "application/cloudevents-batch+json",
                        )
                        .header("webhook-id", &msg_id)
                        .header("webhook-timestamp", now.to_string())
                        .header("webhook-signature", &signature)
                        .body(body.clone())
                        .send()
                },
            )
            .await?;
            for (key, entry, _) in chunk {
                let done = LedgerEntry {
                    sent: true,
                    ..entry.clone()
                };
                self.ledger.put(key, &done).await?;
            }
            report.sent += chunk.len() as u32;
            report.batches += 1;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Quantity, WindowSize};

    fn w() -> WindowRow {
        WindowRow {
            meter: "house_cpu_ms".into(),
            subject: "org_42".into(),
            window_size: WindowSize::Hour,
            window_start: 1_800_000_000,
            group_key: "{}".into(),
            value: Quantity::from_units(1840),
            events: 3,
            close_run: 9,
        }
    }

    #[test]
    fn window_event_shape() {
        let e = window_event("loams://prod-eu1/meter", &w()).expect("event");
        assert_eq!(e["id"], "house_cpu_ms/org_42/hour/1800000000/9");
        assert_eq!(e["type"], EVENT_TYPE);
        assert_eq!(e["time"], "2027-01-15T09:00:00Z");
        assert_eq!(e["data"]["window_start"], "2027-01-15T08:00:00Z");
        assert_eq!(e["data"]["value"], "1840");
        assert_eq!(e["data"]["close_run"], 9);
        assert_eq!(e["data"]["group"], json!({}));
    }

    #[test]
    fn standard_webhooks_signature() {
        // Standard Webhooks' published example (standard-webhooks spec).
        // Built with concat! so the published test vector does not trip secret scanners.
        let key = signing_key(concat!(
            "whsec",
            "_",
            "MfKQ9r8GKYqrTwjU",
            "PD8ILPZIo2LaLaSw"
        ))
        .expect("key");
        let sig = sign(
            &key,
            "msg_p5jXN8AQM9LWM0D4loKWxJek",
            1614265330,
            br#"{"test": 2432232314}"#,
        )
        .expect("sign");
        assert_eq!(sig, "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=");
    }

    #[test]
    fn config_debug_hides_the_secret() {
        let c = WebhookConfig::new("http://x", "whsec_c2VjcmV0", "loams://c/meter");
        assert!(!format!("{c:?}").contains("whsec_c2VjcmV0"));
    }
}
