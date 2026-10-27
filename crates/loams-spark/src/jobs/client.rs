//! A client for the engines HTTP facade's Spark routes (D2006), as the CLI
//! uses them. The facade itself is ENG-UI's (`loams-engines`); the paths are
//! SP-b's assumption in engines-81.md until ENG81 rules:
//!
//! | Call | Route |
//! |---|---|
//! | submit | `POST /v1/engines/spark/jobs` (a [`SparkJobSpec`]) → [`RunStatus`] |
//! | status | `GET /v1/engines/spark/jobs/{run}` → [`RunStatus`] |
//! | list | `GET /v1/engines/spark/jobs?limit=` → `{"runs": [RunStatus]}` |
//! | logs | `GET /v1/engines/spark/jobs/{run}/logs?attempt=` → text |
//! | cancel | `POST /v1/engines/spark/jobs/{run}:cancel` → [`RunStatus`] |
//! | schedule | `POST /v1/engines/spark/schedules` `{"id", "cron", "job"}` |
//! | unschedule | `DELETE /v1/engines/spark/schedules/{id}` |
//! | session | `GET /v1/engines/spark/session` → `{"remote": "sc://…"}` |

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use super::run::RunStatus;
use super::spec::SparkJobSpec;
use super::{JobsError, RunId};

/// The namespace header (the SDK sends the same).
pub const NAMESPACE_HEADER: &str = "x-loams-namespace";

/// A facade client.
#[derive(Clone)]
pub struct SparkClient {
    http: reqwest::Client,
    endpoint: String,
    token: Option<String>,
    namespace: Option<String>,
}

impl std::fmt::Debug for SparkClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SparkClient")
            .field("endpoint", &self.endpoint)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("namespace", &self.namespace)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
struct SessionAnswer {
    remote: String,
}

#[derive(Debug, Deserialize)]
struct RunList {
    runs: Vec<RunStatus>,
}

impl SparkClient {
    /// A client for `endpoint` (`https://…`, or `http://` on loopback).
    pub fn new(
        endpoint: &str,
        token: Option<String>,
        namespace: Option<String>,
    ) -> Result<Self, JobsError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("loams-spark/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| JobsError::Unavailable(e.to_string()))?;
        Ok(Self {
            http,
            endpoint: endpoint.trim_end_matches('/').to_string(),
            token,
            namespace,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1/engines/spark/{path}", self.endpoint)
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.http.request(method, self.url(path));
        if let Some(t) = &self.token {
            r = r.bearer_auth(t);
        }
        if let Some(ns) = &self.namespace {
            r = r.header(NAMESPACE_HEADER, ns);
        }
        r
    }

    async fn send(&self, r: reqwest::RequestBuilder) -> Result<bytes_body::Body, JobsError> {
        let resp = r
            .send()
            .await
            .map_err(|e| JobsError::Unavailable(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| JobsError::Unavailable(e.to_string()))?;
        if status.is_success() {
            return Ok(bytes_body::Body(body.to_vec()));
        }
        let mut message = String::from_utf8_lossy(&body).into_owned();
        // A Connect-style error body carries a message field.
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body)
            && let Some(m) = v.get("message").and_then(|m| m.as_str())
        {
            message = m.to_string();
        }
        message.truncate(1000);
        if status.as_u16() == 404 {
            return Err(JobsError::NotFound(message));
        }
        Err(JobsError::Status {
            status: status.as_u16(),
            message,
        })
    }

    /// Submits a job.
    pub async fn submit(&self, spec: &SparkJobSpec) -> Result<RunStatus, JobsError> {
        let body = self
            .send(self.req(reqwest::Method::POST, "jobs").json(spec))
            .await?;
        body.json()
    }

    /// A run's status.
    pub async fn status(&self, run: &RunId) -> Result<RunStatus, JobsError> {
        self.send(self.req(reqwest::Method::GET, &format!("jobs/{run}")))
            .await?
            .json()
    }

    /// The newest runs.
    pub async fn list(&self, limit: usize) -> Result<Vec<RunStatus>, JobsError> {
        let body = self
            .send(
                self.req(reqwest::Method::GET, "jobs")
                    .query(&[("limit", limit.to_string())]),
            )
            .await?;
        Ok(body.json::<RunList>()?.runs)
    }

    /// An attempt's logs (the last attempt when `attempt` is `None`).
    pub async fn logs(&self, run: &RunId, attempt: Option<u32>) -> Result<String, JobsError> {
        let mut r = self.req(reqwest::Method::GET, &format!("jobs/{run}/logs"));
        if let Some(n) = attempt {
            r = r.query(&[("attempt", n.to_string())]);
        }
        Ok(self.send(r).await?.text())
    }

    /// Cancels a run.
    pub async fn cancel(&self, run: &RunId) -> Result<RunStatus, JobsError> {
        self.send(self.req(reqwest::Method::POST, &format!("jobs/{run}:cancel")))
            .await?
            .json()
    }

    /// Creates or replaces a schedule.
    pub async fn schedule(
        &self,
        id: &str,
        cron: &str,
        job: &SparkJobSpec,
    ) -> Result<serde_json::Value, JobsError> {
        self.send(
            self.req(reqwest::Method::POST, "schedules")
                .json(&json!({ "id": id, "cron": cron, "job": job })),
        )
        .await?
        .json()
    }

    /// Deletes a schedule.
    pub async fn unschedule(&self, id: &str) -> Result<(), JobsError> {
        self.send(self.req(reqwest::Method::DELETE, &format!("schedules/{id}")))
            .await
            .map(|_| ())
    }

    /// The namespace's Spark Connect URL (`sc://…`, never with a token).
    pub async fn session_url(&self) -> Result<String, JobsError> {
        let answer: SessionAnswer = self
            .send(self.req(reqwest::Method::GET, "session"))
            .await?
            .json()?;
        Ok(answer.remote)
    }
}

mod bytes_body {
    use serde::de::DeserializeOwned;

    use crate::jobs::JobsError;

    /// A response body.
    #[derive(Debug)]
    pub struct Body(pub Vec<u8>);

    impl Body {
        pub fn json<T: DeserializeOwned>(&self) -> Result<T, JobsError> {
            serde_json::from_slice(&self.0)
                .map_err(|e| JobsError::Backend(format!("unexpected answer: {e}")))
        }

        pub fn text(self) -> String {
            String::from_utf8_lossy(&self.0).into_owned()
        }
    }
}

/// The Spark Connect URL of a namespace, without asking a server: `sc://<ns>.spark.<domain>:443/;use_ssl=true`
/// in cloud (design 26 §9.2), or the compose proxy's loopback port.
pub fn session_url_for(namespace: &str, domain: Option<&str>) -> String {
    match domain {
        Some(d) => format!("sc://{namespace}.spark.{d}:443/;use_ssl=true"),
        None => "sc://127.0.0.1:15002".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_urls_carry_no_token() {
        assert_eq!(
            session_url_for("acme", Some("loams.dev")),
            "sc://acme.spark.loams.dev:443/;use_ssl=true"
        );
        assert_eq!(session_url_for("acme", None), "sc://127.0.0.1:15002");
    }

    #[test]
    fn debug_redacts_the_token() {
        crate::cli::install_crypto();
        let c = SparkClient::new("http://127.0.0.1:8080/", Some("secret".into()), None)
            .expect("client");
        assert!(!format!("{c:?}").contains("secret"));
        assert_eq!(c.url("jobs"), "http://127.0.0.1:8080/v1/engines/spark/jobs");
    }
}
