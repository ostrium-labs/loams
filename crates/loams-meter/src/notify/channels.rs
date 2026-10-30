//! The notifier's email and console channels (design §63 §15, D2402).
//!
//! - [`ResendMailer`]: email through Resend's HTTP API, the provider Loams'
//!   own mail already uses (Loams Cloud's sending-only key). It sends
//!   plain text from one configured address, with an `Idempotency-Key` per
//!   event so a retry after a lost answer is not a second mail. Recipients
//!   are the rules' addresses, or [`MailRecipients::default_to`] (the org's
//!   owners and admins) when no rule names any.
//! - [`DocConsole`]: the console's meter notifications in a versioned
//!   document beside the other notification records; [`MemConsole`] for
//!   tests and hosts without a control store.
//!
//! Retries are the notifier's: a channel only says whether a send is worth
//! retrying ([`ChannelOutcome`]).

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::notifier::{ChannelOutcome, Console, ConsoleNotification, Mail, Mailer};
use crate::error::{MeterError, Result};
use crate::store::{Customer, VersionedDoc};

/// Resend's API.
pub const RESEND_API: &str = "https://api.resend.com";

/// Resend's most recipients per email.
pub const MAX_RECIPIENTS: usize = 50;

/// Resend's longest idempotency key.
const MAX_IDEMPOTENCY_KEY: usize = 256;

/// The console notifications kept per customer.
pub const CONSOLE_KEPT: usize = 200;

/// Who gets a customer's email when no rule names anyone.
#[async_trait]
pub trait MailRecipients: Send + Sync + fmt::Debug {
    async fn default_to(&self, customer: &Customer) -> Result<Vec<String>>;
}

/// No default recipients: only the rules' addresses get mail.
#[derive(Debug, Clone, Copy, Default)]
pub struct RulesOnly;

#[async_trait]
impl MailRecipients for RulesOnly {
    async fn default_to(&self, _: &Customer) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

/// [`ResendMailer`]'s settings.
#[derive(Clone)]
pub struct ResendConfig {
    /// [`RESEND_API`], or a test server.
    pub api_base: String,
    /// A sending-only API key.
    pub api_key: String,
    /// `Name <address>` or an address on a domain verified in Resend.
    pub from: String,
    /// Per request.
    pub timeout: Duration,
}

impl ResendConfig {
    pub fn new(api_key: impl Into<String>, from: impl Into<String>) -> Self {
        ResendConfig {
            api_base: RESEND_API.to_string(),
            api_key: api_key.into(),
            from: from.into(),
            timeout: Duration::from_secs(10),
        }
    }
}

impl fmt::Debug for ResendConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResendConfig")
            .field("api_base", &self.api_base)
            .field("api_key", &"<redacted>")
            .field("from", &self.from)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// [`Mailer`] over Resend's `POST /emails`.
#[derive(Debug, Clone)]
pub struct ResendMailer {
    client: reqwest::Client,
    cfg: ResendConfig,
    recipients: Arc<dyn MailRecipients>,
}

/// One line, at most 200 characters: provider messages in the delivery log.
fn short(text: &str) -> String {
    let line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    line.chars().take(200).collect()
}

/// `key`, or its SHA-256 when it is longer than Resend takes.
pub fn idempotency_key(key: &str) -> String {
    if key.len() <= MAX_IDEMPOTENCY_KEY {
        key.to_string()
    } else {
        let hex: String = Sha256::digest(key.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("loams:h:{hex}")
    }
}

impl ResendMailer {
    pub fn new(cfg: ResendConfig, recipients: Arc<dyn MailRecipients>) -> Result<Self> {
        if cfg.api_key.trim().is_empty() {
            return Err(MeterError::invalid("mail", "the Resend API key is empty"));
        }
        if cfg.from.trim().is_empty() {
            return Err(MeterError::invalid("mail", "no sender address"));
        }
        let client = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| MeterError::Unavailable(format!("mail client: {e}")))?;
        Ok(ResendMailer {
            client,
            cfg,
            recipients,
        })
    }
}

#[async_trait]
impl Mailer for ResendMailer {
    async fn send(&self, customer: &Customer, mail: &Mail) -> ChannelOutcome {
        let mut to = if mail.to.is_empty() {
            match self.recipients.default_to(customer).await {
                Ok(to) => to,
                Err(e) => return ChannelOutcome::Retry(format!("recipients: {e}")),
            }
        } else {
            mail.to.clone()
        };
        to.sort();
        to.dedup();
        if to.is_empty() {
            return ChannelOutcome::Failed("no recipients".into());
        }
        to.truncate(MAX_RECIPIENTS);
        let body = serde_json::json!({
            "from": self.cfg.from,
            "to": to,
            "subject": mail.subject,
            "text": mail.text,
        });
        let url = format!("{}/emails", self.cfg.api_base.trim_end_matches('/'));
        let answer = self
            .client
            .post(url)
            .bearer_auth(&self.cfg.api_key)
            .header("Idempotency-Key", idempotency_key(&mail.idempotency_key))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await;
        let r = match answer {
            Ok(r) => r,
            Err(e) => return ChannelOutcome::Retry(short(&format!("resend: {e}"))),
        };
        let status = r.status().as_u16();
        let text = r.text().await.unwrap_or_default();
        if (200..300).contains(&status) {
            let id = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string))
                .unwrap_or_else(|| status.to_string());
            return ChannelOutcome::Sent(format!("resend {id}"));
        }
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let m = short(&format!("resend {status}: {message}"));
        // 409: the same idempotency key is still being processed.
        if status >= 500 || matches!(status, 408 | 409 | 429) {
            ChannelOutcome::Retry(m)
        } else {
            ChannelOutcome::Failed(m)
        }
    }
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct ConsoleRecords {
    #[serde(default)]
    notifications: Vec<ConsoleNotification>,
}

impl ConsoleRecords {
    fn put(&mut self, n: ConsoleNotification) {
        if self
            .notifications
            .iter()
            .any(|x| x.id == n.id && x.customer_id == n.customer_id)
        {
            return;
        }
        self.notifications.push(n);
        self.notifications.sort_by_key(|n| std::cmp::Reverse(n.at));
        let mut per: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        self.notifications.retain(|n| {
            let k = per.entry(n.customer_id.clone()).or_insert(0);
            *k += 1;
            *k <= CONSOLE_KEPT
        });
    }

    fn list(&self, customer_id: &str, limit: usize) -> Vec<ConsoleNotification> {
        let mut out: Vec<ConsoleNotification> = self
            .notifications
            .iter()
            .filter(|n| n.customer_id == customer_id)
            .cloned()
            .collect();
        out.sort_by_key(|n| std::cmp::Reverse(n.at));
        out.truncate(limit);
        out
    }
}

/// [`Console`] in memory.
#[derive(Debug, Default)]
pub struct MemConsole {
    inner: Mutex<ConsoleRecords>,
}

impl MemConsole {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Console for MemConsole {
    async fn notify(&self, n: ConsoleNotification) -> Result<()> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .put(n);
        Ok(())
    }

    async fn list(&self, customer_id: &str, limit: usize) -> Result<Vec<ConsoleNotification>> {
        Ok(self
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .list(customer_id, limit))
    }
}

/// [`Console`] on a [`VersionedDoc`], written with compare-and-set like the
/// other meter documents, so every instance shows the same notifications.
pub struct DocConsole {
    doc: Arc<dyn VersionedDoc>,
    writes: tokio::sync::Mutex<()>,
}

impl fmt::Debug for DocConsole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocConsole").finish_non_exhaustive()
    }
}

impl DocConsole {
    pub fn new(doc: Arc<dyn VersionedDoc>) -> Self {
        DocConsole {
            doc,
            writes: tokio::sync::Mutex::new(()),
        }
    }

    async fn current(&self) -> Result<(Option<u64>, ConsoleRecords)> {
        match self.doc.load().await? {
            None => Ok((None, ConsoleRecords::default())),
            Some((v, bytes)) => Ok((
                Some(v),
                serde_json::from_slice(&bytes).map_err(|e| {
                    MeterError::Unavailable(format!("the console notifications do not parse: {e}"))
                })?,
            )),
        }
    }
}

#[async_trait]
impl Console for DocConsole {
    async fn notify(&self, n: ConsoleNotification) -> Result<()> {
        let _one = self.writes.lock().await;
        for _ in 0..8 {
            let (version, mut records) = self.current().await?;
            records.put(n.clone());
            let bytes = serde_json::to_vec(&records).map_err(|e| {
                MeterError::Unavailable(format!("the console notifications do not serialise: {e}"))
            })?;
            if self.doc.save(version, bytes).await?.is_some() {
                return Ok(());
            }
        }
        Err(MeterError::Unavailable(
            "the console notifications changed concurrently too often; retry".into(),
        ))
    }

    async fn list(&self, customer_id: &str, limit: usize) -> Result<Vec<ConsoleNotification>> {
        Ok(self.current().await?.1.list(customer_id, limit))
    }
}
