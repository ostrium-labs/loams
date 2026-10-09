//! `compute_ctl`'s HTTP API, with the per-compute JWT `pg-control` signs
//! (§46 §4.3). Shapes from the fork's `libs/compute_api/src/{requests.rs,
//! responses.rs}` and routes from `compute_tools/src/http/server.rs`.

use reqwest::Method;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::http::Http;
use crate::spec::ComputeSpec;
use crate::{Component, Lsn, NeonError, Secret};

/// A client of one compute's `compute_ctl`.
#[derive(Clone, Debug)]
pub struct ComputeCtlClient {
    http: Http,
}

/// `ComputeStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputeStatus {
    /// Waiting for a spec (a warm-pool compute, §46 §7.3).
    Empty,
    ConfigurationPending,
    Init,
    Running,
    Configuration,
    Failed,
    TerminationPendingFast,
    TerminationPendingImmediate,
    Terminated,
    RefreshConfigurationPending,
    RefreshConfiguration,
}

/// `ComputeStatusResponse`, the fields Loams reads.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ComputeStatusResponse {
    pub status: ComputeStatus,
    pub tenant: Option<String>,
    pub timeline: Option<String>,
    pub error: Option<String>,
}

/// `ComputeCtlConfig`: the keys `compute_ctl` verifies callers' JWTs with,
/// and its TLS settings.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ComputeCtlConfig {
    pub jwks: serde_json::Value,
    pub tls: Option<serde_json::Value>,
}

impl Default for ComputeCtlConfig {
    fn default() -> Self {
        Self {
            jwks: serde_json::json!({ "keys": [] }),
            tls: None,
        }
    }
}

/// `TerminateMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminateMode {
    /// Wait 30 s before answering, so a control plane sees an error.
    Fast,
    /// Answer once everything has stopped.
    Immediate,
}

#[derive(Serialize)]
struct ConfigurationRequest<'a> {
    spec: &'a ComputeSpec,
    compute_ctl_config: &'a ComputeCtlConfig,
}

#[derive(Serialize)]
struct PromoteConfig<'a> {
    spec: &'a ComputeSpec,
    wal_flush_lsn: Lsn,
}

/// `PromoteState`: `completed` (with its timings), `failed` (a 500 with
/// `error`) or `not_promoted`.
#[derive(Deserialize)]
struct PromoteState {
    status: String,
}

#[derive(Deserialize)]
struct TerminateResponse {
    lsn: Option<Lsn>,
}

impl ComputeCtlClient {
    /// `jwt` is the compute's own token, signed by `pg-control`.
    pub fn new(base: Url, jwt: Secret<String>) -> Self {
        Self {
            http: Http::new(Component::ComputeCtl, base, Some(jwt)),
        }
    }

    /// `GET /status`.
    pub async fn status(&self) -> Result<ComputeStatusResponse, NeonError> {
        self.http.get("/status").await
    }

    /// `POST /configure`: hand a waiting compute its spec, or reconfigure a
    /// running one.
    pub async fn configure(
        &self,
        spec: &ComputeSpec,
        config: &ComputeCtlConfig,
    ) -> Result<ComputeStatusResponse, NeonError> {
        let body = ConfigurationRequest {
            spec,
            compute_ctl_config: config,
        };
        self.http.json(Method::POST, "/configure", &body).await
    }

    /// `POST /terminate?mode=`: the LSN Postgres stopped at, when it knows it.
    pub async fn terminate(&self, mode: TerminateMode) -> Result<Option<Lsn>, NeonError> {
        let req = self
            .http
            .request(Method::POST, "/terminate")?
            .query(&[("mode", mode)]);
        let resp: TerminateResponse = self.http.send(req).await?;
        Ok(resp.lsn)
    }

    /// `POST /promote`: a replica becomes the primary, with `spec` (in
    /// `Primary` mode) once its WAL reaches `wal_flush_lsn`. `Ok` only when
    /// `compute_ctl` answers `completed`; a failed promotion is its 500 with
    /// the error as `msg`.
    pub async fn promote(&self, spec: &ComputeSpec, wal_flush_lsn: Lsn) -> Result<(), NeonError> {
        let body = PromoteConfig {
            spec,
            wal_flush_lsn,
        };
        let state: PromoteState = self.http.json(Method::POST, "/promote", &body).await?;
        if state.status == "completed" {
            Ok(())
        } else {
            Err(NeonError {
                status: 0,
                msg: format!("promotion answered {:?}", state.status),
                component: Component::ComputeCtl,
            })
        }
    }
}
