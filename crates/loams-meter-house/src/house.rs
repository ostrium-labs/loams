//! The House, as the meter reaches it: SQL over the ClickHouse HTTP
//! interface, with the meter's own House user (mapped to the system
//! namespace by the operator, §63 §7.5).

use std::future::Future;
use std::time::Duration;

/// A House request that failed.
#[derive(Debug, thiserror::Error)]
pub enum HouseError {
    /// The House answered with an error: its ClickHouse code and message.
    #[error("House error {code}: {message}")]
    Query { code: u32, message: String },
    /// The House could not be reached, or its answer was cut off.
    #[error("House unreachable: {0}")]
    Transport(String),
    /// The House answered with something the meter cannot read.
    #[error("unexpected House answer: {0}")]
    Decode(String),
}

/// Runs SQL in the House. One statement per call; the answer is the
/// statement's output (`TabSeparated` unless the statement says `FORMAT`).
pub trait HouseSql: Send + Sync {
    fn query(&self, sql: &str) -> impl Future<Output = Result<String, HouseError>> + Send;

    /// `sql` with Loams settings, such as `loams_snapshot_id`, which the
    /// House takes only from the request (URL parameters), never from a
    /// statement's `SETTINGS` clause (that clause is chDB's). A House that
    /// cannot pass them refuses rather than run the statement without them.
    fn query_with(
        &self,
        sql: &str,
        settings: &[(&str, String)],
    ) -> impl Future<Output = Result<String, HouseError>> + Send {
        let _ = (sql, settings);
        async {
            Err(HouseError::Transport(
                "this House client cannot pass Loams settings".into(),
            ))
        }
    }
}

/// `text` percent-encoded for a URL query (unreserved characters kept).
fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// [`HouseSql`] over the House's HTTP listener.
#[derive(Clone)]
pub struct HttpHouse {
    client: reqwest::Client,
    url: String,
    user: String,
    password: String,
}

impl std::fmt::Debug for HttpHouse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpHouse")
            .field("url", &self.url)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

impl HttpHouse {
    /// The House at `url` (`http://house:8123`), as `user`.
    pub fn new(
        url: impl Into<String>,
        user: impl Into<String>,
        password: impl Into<String>,
    ) -> Result<Self, HouseError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| HouseError::Transport(e.to_string()))?;
        Ok(HttpHouse {
            client,
            url: url.into().trim_end_matches('/').to_string(),
            user: user.into(),
            password: password.into(),
        })
    }
}

impl HouseSql for HttpHouse {
    async fn query(&self, sql: &str) -> Result<String, HouseError> {
        self.query_with(sql, &[]).await
    }

    async fn query_with(
        &self,
        sql: &str,
        settings: &[(&str, String)],
    ) -> Result<String, HouseError> {
        let params: Vec<String> = settings
            .iter()
            .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
            .collect();
        let url = if params.is_empty() {
            format!("{}/", self.url)
        } else {
            format!("{}/?{}", self.url, params.join("&"))
        };
        let response = self
            .client
            .post(url)
            .basic_auth(&self.user, Some(&self.password))
            .body(sql.to_string())
            .send()
            .await
            .map_err(|e| HouseError::Transport(e.to_string()))?;
        let status = response.status();
        let code = response
            .headers()
            .get("x-clickhouse-exception-code")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let body = response
            .text()
            .await
            .map_err(|e| HouseError::Transport(e.to_string()))?;
        if status.is_success() && code.is_none() {
            Ok(body)
        } else {
            Err(HouseError::Query {
                code: code.unwrap_or(0),
                message: body.trim().to_string(),
            })
        }
    }
}

/// Sets the meter's House tables up ([`crate::sql::schema`]). Idempotent:
/// the `meter` role calls it at every start.
pub async fn ensure_schema(house: &impl HouseSql) -> Result<(), HouseError> {
    for statement in crate::sql::schema() {
        house.query(&statement).await?;
    }
    Ok(())
}
