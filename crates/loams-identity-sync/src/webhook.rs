//! The wake-up webhook (design 76 §8.1, D1622, D1627).
//!
//! Zitadel's Actions v2 call a target with a `ZITADEL-Signature` header,
//! `t=<unix seconds>,v1=<hex HMAC-SHA256>`, where the MAC is over `<t>.<body>` with the
//! target's signing key (Zitadel's documentation and its Apache-2.0 Go SDK). A call is
//! delivered at most once, so its body is never applied: a call that verifies only makes the
//! sync read the event list now. A call older or newer than [`TOLERANCE`], or signed with no
//! key given, is refused.

use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tokio::sync::Notify;

/// The header Zitadel signs its calls in.
pub const SIGNATURE_HEADER: &str = "ZITADEL-Signature";

/// How far a call's timestamp may be from now, either way (Zitadel's SDK uses 300 s).
pub const TOLERANCE: Duration = Duration::from_secs(300);

/// The largest body the endpoint reads; anything longer is refused before it is verified.
pub const MAX_BODY: usize = 1 << 20;

/// Why a call was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    #[error("no ZITADEL-Signature header")]
    NotSigned,
    #[error("the ZITADEL-Signature header is malformed")]
    Malformed,
    #[error("the call's timestamp is outside the tolerance")]
    Stale,
    #[error("no signature matches a signing key")]
    BadSignature,
    #[error("the body is too large")]
    TooLarge,
}

/// Verifies `header` over `body` with any of `keys` (several during a key rotation), at
/// `now` (unix seconds).
///
/// # Errors
///
/// [`Refused`] says why.
pub fn verify(header: Option<&str>, body: &[u8], keys: &[String], now: i64) -> Result<(), Refused> {
    if body.len() > MAX_BODY {
        return Err(Refused::TooLarge);
    }
    let header = header.filter(|h| !h.is_empty()).ok_or(Refused::NotSigned)?;
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        let (name, value) = part.trim().split_once('=').ok_or(Refused::Malformed)?;
        match name {
            "t" => timestamp = Some(value.parse::<i64>().map_err(|_| Refused::Malformed)?),
            "v1" => {
                if let Ok(sig) = hex::decode(value) {
                    signatures.push(sig);
                }
            }
            _ => {}
        }
    }
    let t = timestamp.ok_or(Refused::Malformed)?;
    if signatures.is_empty() {
        return Err(Refused::BadSignature);
    }
    if now.abs_diff(t) > TOLERANCE.as_secs() {
        return Err(Refused::Stale);
    }
    for key in keys.iter().filter(|k| !k.is_empty()) {
        for sig in &signatures {
            let Ok(mut mac) = <Hmac<Sha256> as KeyInit>::new_from_slice(key.as_bytes()) else {
                continue;
            };
            mac.update(t.to_string().as_bytes());
            mac.update(b".");
            mac.update(body);
            // Constant time.
            if mac.verify_slice(sig).is_ok() {
                return Ok(());
            }
        }
    }
    Err(Refused::BadSignature)
}

/// The endpoint's logic, for whichever HTTP server mounts it: verify, then wake the sync.
#[derive(Clone)]
pub struct Wake {
    keys: Arc<Vec<String>>,
    notify: Arc<Notify>,
}

impl std::fmt::Debug for Wake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wake")
            .field("keys", &self.keys.len())
            .finish_non_exhaustive()
    }
}

impl Wake {
    /// `keys` are the targets' signing keys; `notify` is [`crate::IdentitySync::waker`].
    pub fn new(keys: Vec<String>, notify: Arc<Notify>) -> Self {
        Self {
            keys: Arc::new(keys),
            notify,
        }
    }

    /// The HTTP status to answer: 200 after waking the sync, 401 for a call that does not
    /// verify, 413 for a body over [`MAX_BODY`]. The body is not read beyond its MAC.
    pub fn handle(&self, header: Option<&str>, body: &[u8]) -> u16 {
        match verify(header, body, &self.keys, crate::now().unix_timestamp()) {
            Ok(()) => {
                self.notify.notify_one();
                200
            }
            Err(Refused::TooLarge) => 413,
            Err(why) => {
                tracing::warn!(target: "loams_identity_sync", %why, "refused a wake-up call");
                401
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn sign(t: i64, body: &[u8], key: &str) -> String {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key.as_bytes()).unwrap();
        mac.update(format!("{t}.").as_bytes());
        mac.update(body);
        format!("t={t},v1={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn a_signed_call_verifies() {
        let keys = vec!["k1".to_owned()];
        let h = sign(1_000, b"{}", "k1");
        assert_eq!(verify(Some(&h), b"{}", &keys, 1_000), Ok(()));
        assert_eq!(verify(Some(&h), b"{}", &keys, 1_299), Ok(()));
    }

    #[test]
    fn a_rotated_key_verifies_with_either() {
        let keys = vec!["old".to_owned(), "new".to_owned()];
        let h = format!("{},v1={}", sign(5, b"x", "old"), "zz");
        assert_eq!(verify(Some(&h), b"x", &keys, 5), Ok(()));
    }

    #[test]
    fn refusals() {
        let keys = vec!["k1".to_owned()];
        let h = sign(1_000, b"{}", "k1");
        assert_eq!(verify(None, b"{}", &keys, 1_000), Err(Refused::NotSigned));
        assert_eq!(
            verify(Some("nonsense"), b"{}", &keys, 1_000),
            Err(Refused::Malformed)
        );
        assert_eq!(
            verify(Some(&h), b"{ }", &keys, 1_000),
            Err(Refused::BadSignature)
        );
        assert_eq!(
            verify(Some(&h), b"{}", &["k2".into()], 1_000),
            Err(Refused::BadSignature)
        );
        assert_eq!(
            verify(Some(&h), b"{}", &[String::new()], 1_000),
            Err(Refused::BadSignature)
        );
        assert_eq!(verify(Some(&h), b"{}", &keys, 1_301), Err(Refused::Stale));
        assert_eq!(
            verify(Some(&h), b"{}", &keys, 699),
            Err(Refused::Stale),
            "from the future"
        );
        assert_eq!(
            verify(Some("t=1000"), b"{}", &keys, 1_000),
            Err(Refused::BadSignature)
        );
        let big = vec![b'a'; MAX_BODY + 1];
        assert_eq!(verify(Some(&h), &big, &keys, 1_000), Err(Refused::TooLarge));
    }

    #[tokio::test]
    async fn webhook_body_not_applied() {
        // A verified call only wakes the sync: whatever its body says, nothing else happens.
        let notify = Arc::new(Notify::new());
        let wake = Wake::new(vec!["k".into()], notify.clone());
        let now = crate::now().unix_timestamp();
        let body = br#"{"event_type":"user.grant.added","event_payload":{"roleKeys":["owner"]}}"#;
        assert_eq!(wake.handle(Some(&sign(now, body, "k")), body), 200);
        tokio::time::timeout(Duration::from_secs(1), notify.notified())
            .await
            .unwrap();
        assert_eq!(wake.handle(Some(&sign(now, body, "x")), body), 401);
    }
}
