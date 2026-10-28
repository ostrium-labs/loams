//! A client for the engines HTTP facade's Spark routes, as the CLI uses
//! them. The facade is ENG-UI's (`loams-engines`, D2015); ENG81 R2 and R15 put
//! every route under one engine instance of one namespace,
//! `/v1/namespaces/{ns}/engines/{instance}`:
//!
//! | Call | Route |
//! |---|---|
//! | submit | `POST …/jobs` (a [`SparkJobSpec`]) → [`RunStatus`] |
//! | list | `GET …/jobs?limit=` → `{"jobs": [RunStatus]}` |
//! | cancel | `POST …/jobs/{run}/cancel` → [`RunStatus`] |
//! | session | `POST …/sessions` `{"kind": "python"}` → `{"connect_url": "sc://…", …}` |
//!
//! and three SP-b proposes to ENG-UI (engines-81.md, SP-b round 2):
//!
//! | Call | Route |
//! |---|---|
//! | status | `GET …/jobs/{run}` → [`RunStatus`] |
//! | logs | `GET …/jobs/{run}/logs?attempt=` → text |
//! | schedule | `POST …/schedules` `{"id", "cron", "job"}`; `DELETE …/schedules/{id}` |

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
    namespace: String,
    instance: String,
}

impl std::fmt::Debug for SparkClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SparkClient")
            .field("endpoint", &self.endpoint)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("namespace", &self.namespace)
            .field("instance", &self.instance)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
struct SessionAnswer {
    #[serde(alias = "remote")]
    connect_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RunList {
    Jobs { jobs: Vec<RunStatus> },
    Runs { runs: Vec<RunStatus> },
    Bare(Vec<RunStatus>),
}

/// A URL path segment, percent-encoded.
fn segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl SparkClient {
    /// A client for one Spark engine instance of a namespace, at `endpoint`
    /// (`https://…`, or `http://` on loopback).
    pub fn new(
        endpoint: &str,
        token: Option<String>,
        namespace: &str,
        instance: &str,
    ) -> Result<Self, JobsError> {
        if namespace.is_empty() || instance.is_empty() {
            return Err(JobsError::InvalidSpec(
                "a namespace (--namespace, LOAMS_NAMESPACE) and a Spark instance (--instance, \
                 LOAMS_SPARK_INSTANCE) are both needed"
                    .to_string(),
            ));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("loams-spark/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| JobsError::Unavailable(e.to_string()))?;
        Ok(Self {
            http,
            endpoint: endpoint.trim_end_matches('/').to_string(),
            token,
            namespace: namespace.to_string(),
            instance: instance.to_string(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/v1/namespaces/{}/engines/{}/{path}",
            self.endpoint,
            segment(&self.namespace),
            segment(&self.instance)
        )
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.http.request(method, self.url(path));
        if let Some(t) = &self.token {
            r = r.bearer_auth(t);
        }
        r.header(NAMESPACE_HEADER, &self.namespace)
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
        Ok(match body.json::<RunList>()? {
            RunList::Jobs { jobs } => jobs,
            RunList::Runs { runs } => runs,
            RunList::Bare(runs) => runs,
        })
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
        self.send(self.req(reqwest::Method::POST, &format!("jobs/{run}/cancel")))
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
        self.send(self.req(
            reqwest::Method::DELETE,
            &format!("schedules/{}", segment(id)),
        ))
        .await
        .map(|_| ())
    }

    /// Opens a session on the instance and answers its Spark Connect URL
    /// (`sc://…`, never with a token).
    pub async fn session_url(&self) -> Result<String, JobsError> {
        let answer: SessionAnswer = self
            .send(
                self.req(reqwest::Method::POST, "sessions")
                    .json(&json!({ "kind": "python", "conf": {} })),
            )
            .await?
            .json()?;
        Ok(answer.connect_url)
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

/// The Spark Connect URL of an instance, without asking a server (ENG81
/// R28): `sc://<instance>.<ns>.<region>.spark.<base-domain>:443/;use_ssl=true`
/// in cloud, or `engine-gateway`'s loopback port on compose (R7).
pub fn session_url_for(
    instance: &str,
    namespace: &str,
    region_and_domain: Option<(&str, &str)>,
) -> String {
    match region_and_domain {
        Some((region, domain)) => {
            format!("sc://{instance}.{namespace}.{region}.spark.{domain}:443/;use_ssl=true")
        }
        None => "sc://127.0.0.1:15002".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_urls_carry_no_token() {
        assert_eq!(
            session_url_for("etl", "acme", Some(("eu1", "loams.dev"))),
            "sc://etl.acme.eu1.spark.loams.dev:443/;use_ssl=true"
        );
        assert_eq!(session_url_for("etl", "acme", None), "sc://127.0.0.1:15002");
    }

    #[test]
    fn debug_redacts_the_token() {
        crate::cli::install_crypto();
        let c = SparkClient::new(
            "http://127.0.0.1:8080/",
            Some("secret".into()),
            "acme",
            "etl",
        )
        .expect("client");
        assert!(!format!("{c:?}").contains("secret"));
        assert_eq!(
            c.url("jobs"),
            "http://127.0.0.1:8080/v1/namespaces/acme/engines/etl/jobs"
        );
        assert!(SparkClient::new("http://x", None, "", "etl").is_err());
    }

    #[test]
    fn run_lists_in_any_shape() {
        let one = r#"{"run_id": "01jbzq3w4y5x6v7t8s9r0pqmnk", "name": "etl", "state": "running",
            "attempt": 1, "max_attempts": 1, "backend": "connect",
            "created_at": "2026-10-27T00:00:00Z", "updated_at": "2026-10-27T00:00:00Z"}"#;
        for body in [
            format!(r#"{{"jobs": [{one}]}}"#),
            format!(r#"{{"runs": [{one}]}}"#),
            format!("[{one}]"),
        ] {
            let list: RunList = serde_json::from_str(&body).expect("parse");
            let n = match list {
                RunList::Jobs { jobs } => jobs.len(),
                RunList::Runs { runs } => runs.len(),
                RunList::Bare(runs) => runs.len(),
            };
            assert_eq!(n, 1);
        }
    }
}
