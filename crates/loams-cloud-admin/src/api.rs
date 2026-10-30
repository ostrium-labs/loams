//! The control plane's admin API under `/cloud/v1/admin` (design 78 §25.10).

use serde::Deserialize;
use serde_json::{Value, json};

use crate::Error;

/// One admin session against one control plane.
#[derive(Debug, Clone)]
pub struct AdminApi {
    http: reqwest::Client,
    base: String,
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: ErrorInner,
}

#[derive(Debug, Deserialize)]
struct ErrorInner {
    code: String,
    message: String,
    #[serde(default)]
    request_id: Option<String>,
}

/// Query parameters for the waitlist list.
#[derive(Debug, Clone, Default)]
pub struct WaitlistQuery {
    pub status: Option<String>,
    pub since: Option<String>,
    pub source: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// Query parameters for the audit list.
#[derive(Debug, Clone, Default)]
pub struct AuditQuery {
    pub subject: Option<String>,
    pub r#type: Option<String>,
    pub since: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

fn push(q: &mut Vec<(&'static str, String)>, k: &'static str, v: Option<&String>) {
    if let Some(v) = v {
        q.push((k, v.clone()));
    }
}

/// Whether `base` may carry a credential: HTTPS, or plain HTTP to a loopback
/// address (the in-process fakes the tests start).
fn credential_safe(base: &str) -> bool {
    if base.starts_with("https://") {
        return true;
    }
    let Some(rest) = base.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl AdminApi {
    /// `base` is the API root, `https://api.loams.dev/cloud/v1` outside tests.
    #[must_use]
    pub fn new(
        http: reqwest::Client,
        base: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Self {
        Self {
            http,
            base: base.into().trim_end_matches('/').to_owned(),
            access_token: access_token.into(),
        }
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&'static str, String)],
        body: Option<Value>,
        idempotency_key: Option<&str>,
    ) -> Result<Value, Error> {
        if !credential_safe(&self.base) {
            return Err(Error::Usage(format!(
                "refusing to send the admin token to {} over plain HTTP; use https://",
                self.base
            )));
        }
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.access_token)
            .query(query);
        if let Some(k) = idempotency_key {
            req = req.header("idempotency-key", k);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let r = req.send().await?;
        let status = r.status();
        let text = r.text().await?;
        if status.is_success() {
            return Ok(if text.is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&text)?
            });
        }
        match serde_json::from_str::<ErrorBody>(&text) {
            Ok(e) => Err(Error::Api {
                status: status.as_u16(),
                code: e.error.code,
                message: e.error.message,
                request_id: e.error.request_id,
            }),
            Err(_) => Err(Error::Api {
                status: status.as_u16(),
                code: "HTTP".into(),
                message: status.to_string(),
                request_id: None,
            }),
        }
    }

    /// `POST /admin/mfa/enroll`.
    ///
    /// # Errors
    ///
    /// `ALREADY_ENROLLED`, an auth failure, or a network error.
    pub async fn mfa_enroll(&self) -> Result<Value, Error> {
        self.call(reqwest::Method::POST, "/admin/mfa/enroll", &[], None, None)
            .await
    }

    /// `POST /admin/mfa/verify`.
    ///
    /// # Errors
    ///
    /// `CODE_INVALID`, an auth failure, or a network error.
    pub async fn mfa_verify(&self, code: &str) -> Result<Value, Error> {
        self.call(
            reqwest::Method::POST,
            "/admin/mfa/verify",
            &[],
            Some(json!({ "code": code })),
            None,
        )
        .await
    }

    /// `GET /admin/waitlist`.
    ///
    /// # Errors
    ///
    /// An auth failure, a bad parameter, or a network error.
    pub async fn waitlist(&self, q: &WaitlistQuery) -> Result<Value, Error> {
        let mut query = Vec::new();
        push(&mut query, "status", q.status.as_ref());
        push(&mut query, "since", q.since.as_ref());
        push(&mut query, "source", q.source.as_ref());
        push(&mut query, "cursor", q.cursor.as_ref());
        push(&mut query, "limit", q.limit.map(|l| l.to_string()).as_ref());
        self.call(reqwest::Method::GET, "/admin/waitlist", &query, None, None)
            .await
    }

    /// `DELETE /admin/waitlist/{id}`.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND`, an auth failure, or a network error.
    pub async fn waitlist_delete(&self, id: &str) -> Result<(), Error> {
        self.call(
            reqwest::Method::DELETE,
            &format!("/admin/waitlist/{id}"),
            &[],
            None,
            None,
        )
        .await
        .map(|_| ())
    }

    /// `POST /admin/invites`, with one Idempotency-Key for the batch: a retry
    /// after a lost response returns the same answer instead of a second batch.
    ///
    /// # Errors
    ///
    /// `DAILY_SEND_CAP`, a bad batch, an auth failure, or a network error after the retries.
    pub async fn invite(
        &self,
        waitlist_ids: &[String],
        expires_in_days: Option<u8>,
        idempotency_key: &str,
    ) -> Result<Value, Error> {
        let mut body = json!({ "waitlist_ids": waitlist_ids });
        if let Some(d) = expires_in_days {
            body["expires_in_days"] = json!(d);
        }
        let mut attempt = 0u64;
        loop {
            match self
                .call(
                    reqwest::Method::POST,
                    "/admin/invites",
                    &[],
                    Some(body.clone()),
                    Some(idempotency_key),
                )
                .await
            {
                Err(Error::Http(_)) if attempt < 2 => {
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(200 * attempt)).await;
                }
                other => return other,
            }
        }
    }

    /// `GET /admin/invites`.
    ///
    /// # Errors
    ///
    /// An auth failure, a bad parameter, or a network error.
    pub async fn invites(
        &self,
        state: Option<&String>,
        cursor: Option<&String>,
        limit: Option<u32>,
    ) -> Result<Value, Error> {
        let mut query = Vec::new();
        push(&mut query, "state", state);
        push(&mut query, "cursor", cursor);
        push(&mut query, "limit", limit.map(|l| l.to_string()).as_ref());
        self.call(reqwest::Method::GET, "/admin/invites", &query, None, None)
            .await
    }

    /// `POST /admin/invites/{id}/revoke`.
    ///
    /// # Errors
    ///
    /// `INVITE_NOT_REVOCABLE`, `NOT_FOUND`, an auth failure, or a network error.
    pub async fn revoke(&self, id: &str) -> Result<Value, Error> {
        self.call(
            reqwest::Method::POST,
            &format!("/admin/invites/{id}/revoke"),
            &[],
            None,
            None,
        )
        .await
    }

    /// `POST /admin/invites/{id}/resend`.
    ///
    /// # Errors
    ///
    /// `RESEND_LIMIT`, `INVITE_NOT_RESENDABLE`, `DAILY_SEND_CAP`, an auth failure, or a network error.
    pub async fn resend(&self, id: &str, extend: bool) -> Result<Value, Error> {
        self.call(
            reqwest::Method::POST,
            &format!("/admin/invites/{id}/resend"),
            &[],
            Some(json!({ "extend": extend })),
            None,
        )
        .await
    }

    /// `GET /admin/audit`.
    ///
    /// # Errors
    ///
    /// An auth failure, a bad parameter, or a network error.
    pub async fn audit(&self, q: &AuditQuery) -> Result<Value, Error> {
        let mut query = Vec::new();
        push(&mut query, "subject", q.subject.as_ref());
        push(&mut query, "type", q.r#type.as_ref());
        push(&mut query, "since", q.since.as_ref());
        push(&mut query, "cursor", q.cursor.as_ref());
        push(&mut query, "limit", q.limit.map(|l| l.to_string()).as_ref());
        self.call(reqwest::Method::GET, "/admin/audit", &query, None, None)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_go_only_over_https_or_loopback() {
        for ok in [
            "https://api.example.com",
            "http://127.0.0.1:9",
            "http://localhost:8080/v1",
            "http://[::1]:80",
        ] {
            assert!(credential_safe(ok), "{ok}");
        }
        for bad in [
            "http://api.example.com",
            "http://10.0.0.1:80",
            "http://127.0.0.1.evil.example",
            "ftp://127.0.0.1",
        ] {
            assert!(!credential_safe(bad), "{bad}");
        }
    }
}
