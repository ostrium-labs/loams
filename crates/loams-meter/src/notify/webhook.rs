//! Signed webhooks (design §63 §15): CloudEvents in structured mode, signed
//! as Standard Webhooks specifies, so any of its verifier libraries checks
//! them:
//!
//! - `webhook-id`: the event id (the receiver's dedup key);
//! - `webhook-timestamp`: Unix seconds;
//! - `webhook-signature`: `v1,<base64 HMAC-SHA256(secret, "<id>.<timestamp>.<body>")>`,
//!   space-separated when a rotated secret is still valid.
//!
//! The secret is `whsec_<base64 of 32 random bytes>`, shown once when the
//! endpoint is created or its secret rotated.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::error::{MeterError, Result};

/// The secret prefix.
pub const SECRET_PREFIX: &str = "whsec_";

/// How far a receiver should accept a timestamp from its clock.
pub const TOLERANCE: Duration = Duration::from_secs(5 * 60);

/// When to retry a failed delivery, after the first attempt: Standard
/// Webhooks' suggested schedule (5 s, 5 min, 30 min, 2 h, 5 h, 10 h).
pub const RETRY_AFTER: &[Duration] = &[
    Duration::from_secs(5),
    Duration::from_secs(5 * 60),
    Duration::from_secs(30 * 60),
    Duration::from_secs(2 * 3600),
    Duration::from_secs(5 * 3600),
    Duration::from_secs(10 * 3600),
];

/// A new secret from 32 random bytes.
pub fn new_secret(random: [u8; 32]) -> String {
    format!("{SECRET_PREFIX}{}", BASE64.encode(random))
}

fn key_of(secret: &str) -> Result<Vec<u8>> {
    let b64 = secret
        .strip_prefix(SECRET_PREFIX)
        .ok_or_else(|| MeterError::invalid("secret", "a webhook secret starts with whsec_"))?;
    BASE64
        .decode(b64)
        .map_err(|_| MeterError::invalid("secret", "the secret is not base64"))
}

/// The `v1,<signature>` of one message.
pub fn sign(secret: &str, msg_id: &str, timestamp: i64, body: &[u8]) -> Result<String> {
    let key = key_of(secret)?;
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(&key).expect("HMAC takes any key length");
    mac.update(msg_id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    Ok(format!("v1,{}", BASE64.encode(mac.finalize().into_bytes())))
}

/// Checks a `webhook-signature` header against `secret` (what a receiver
/// does; used by the tests and by anyone verifying with this crate).
pub fn verify(
    secret: &str,
    msg_id: &str,
    timestamp: i64,
    body: &[u8],
    header: &str,
    now_unix: i64,
) -> bool {
    if (now_unix - timestamp).unsigned_abs() > TOLERANCE.as_secs() {
        return false;
    }
    let Ok(key) = key_of(secret) else {
        return false;
    };
    header.split(' ').any(|sig| {
        let Some(b64) = sig.strip_prefix("v1,") else {
            return false;
        };
        let Ok(given) = BASE64.decode(b64) else {
            return false;
        };
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&key).expect("any key length");
        mac.update(msg_id.as_bytes());
        mac.update(b".");
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        mac.verify_slice(&given).is_ok()
    })
}

/// Where a webhook may point: `https`, or `http` on loopback only (local
/// development). No credentials in the URL.
pub fn check_url(url: &str) -> Result<()> {
    let rest = if let Some(r) = url.strip_prefix("https://") {
        r
    } else if let Some(r) = url.strip_prefix("http://") {
        let host = r.split(['/', ':']).next().unwrap_or("");
        if !matches!(host, "localhost" | "127.0.0.1") && !r.starts_with("[::1]") {
            return Err(MeterError::invalid(
                "url",
                "a webhook URL must be https (http only on loopback)",
            ));
        }
        r
    } else {
        return Err(MeterError::invalid("url", "a webhook URL must be https"));
    };
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || url.len() > 2048 {
        return Err(MeterError::invalid(
            "url",
            "a webhook URL needs a host and no credentials",
        ));
    }
    Ok(())
}

/// One HTTP attempt's outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A 2xx answer.
    Delivered(u16),
    /// Another status: retried, except 410 Gone, which disables nothing but
    /// stops this delivery.
    Status(u16),
    /// No answer (connect, TLS, timeout).
    Failed(String),
}

impl Outcome {
    pub fn is_success(&self) -> bool {
        matches!(self, Outcome::Delivered(_))
    }

    /// Worth retrying.
    pub fn retryable(&self) -> bool {
        match self {
            Outcome::Delivered(_) => false,
            Outcome::Status(s) => *s != 410 && (*s >= 500 || *s == 408 || *s == 429),
            Outcome::Failed(_) => true,
        }
    }
}

/// Sends one signed request.
#[async_trait]
pub trait Transport: Send + Sync + fmt::Debug {
    async fn post(&self, url: &str, headers: &[(&str, String)], body: &[u8]) -> Outcome;
}

/// [`Transport`] over reqwest: 10 s per attempt, no redirects (a redirect
/// could carry the signature elsewhere).
#[derive(Debug, Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
}

impl HttpTransport {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| MeterError::Unavailable(format!("webhook client: {e}")))?;
        Ok(HttpTransport { client })
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn post(&self, url: &str, headers: &[(&str, String)], body: &[u8]) -> Outcome {
        let mut req = self.client.post(url).body(body.to_vec());
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        match req.send().await {
            Ok(r) if r.status().is_success() => Outcome::Delivered(r.status().as_u16()),
            Ok(r) => Outcome::Status(r.status().as_u16()),
            Err(e) => Outcome::Failed(e.to_string()),
        }
    }
}

/// The headers of one signed delivery.
pub fn headers(
    secrets: &[String],
    msg_id: &str,
    timestamp: i64,
    body: &[u8],
) -> Result<Vec<(&'static str, String)>> {
    let mut sigs = Vec::new();
    for s in secrets {
        sigs.push(sign(s, msg_id, timestamp, body)?);
    }
    Ok(vec![
        ("content-type", "application/cloudevents+json".to_string()),
        ("webhook-id", msg_id.to_string()),
        ("webhook-timestamp", timestamp.to_string()),
        ("webhook-signature", sigs.join(" ")),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_verify_and_tampering_does_not() {
        let secret = new_secret([7; 32]);
        let body = br#"{"id":"e1"}"#;
        let sig = sign(&secret, "e1", 1_700_000_000, body).expect("sign");
        assert!(sig.starts_with("v1,"));
        assert!(verify(
            &secret,
            "e1",
            1_700_000_000,
            body,
            &sig,
            1_700_000_010
        ));
        assert!(!verify(
            &secret,
            "e1",
            1_700_000_000,
            b"{}",
            &sig,
            1_700_000_010
        ));
        assert!(!verify(
            &secret,
            "e2",
            1_700_000_000,
            body,
            &sig,
            1_700_000_010
        ));
        assert!(!verify(
            &new_secret([8; 32]),
            "e1",
            1_700_000_000,
            body,
            &sig,
            1_700_000_010
        ));
        assert!(
            !verify(&secret, "e1", 1_700_000_000, body, &sig, 1_700_001_000),
            "too old"
        );
        // A rotated secret: both signatures are sent, either verifies.
        let old = new_secret([1; 32]);
        let h =
            headers(&[secret.clone(), old.clone()], "e1", 1_700_000_000, body).expect("headers");
        let both = &h
            .iter()
            .find(|(k, _)| *k == "webhook-signature")
            .expect("sig")
            .1;
        assert!(verify(&old, "e1", 1_700_000_000, body, both, 1_700_000_000));
        assert!(verify(
            &secret,
            "e1",
            1_700_000_000,
            body,
            both,
            1_700_000_000
        ));
    }

    #[test]
    fn webhook_urls_are_https() {
        check_url("https://hooks.example.com/meter").expect("https");
        check_url("http://localhost:8080/hook").expect("loopback");
        check_url("http://127.0.0.1/hook").expect("loopback");
        assert!(check_url("http://example.com/hook").is_err());
        assert!(check_url("https://user:pw@example.com/").is_err());
        assert!(check_url("ftp://example.com").is_err());
    }

    #[test]
    fn retries_are_for_transient_failures() {
        assert!(Outcome::Status(503).retryable());
        assert!(Outcome::Status(429).retryable());
        assert!(Outcome::Failed("reset".into()).retryable());
        assert!(!Outcome::Status(400).retryable());
        assert!(!Outcome::Status(410).retryable());
        assert!(!Outcome::Delivered(204).retryable());
    }
}
