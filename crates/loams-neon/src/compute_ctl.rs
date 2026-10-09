//! `compute_ctl`'s HTTP API, with the per-compute JWT `pg-control` signs
//! (§46 §4.3). Shapes from the fork's `libs/compute_api/src/{requests.rs,
//! responses.rs}` and routes from `compute_tools/src/http/server.rs`.
//!
//! `configure`, `terminate` and `promote` block in `compute_ctl` until the
//! compute gets there, so they have long timeouts of their own. A timeout
//! does not mean the call failed: the caller polls [`ComputeCtlClient::status`]
//! and repeats the call if the compute did not move (each of the three is
//! safe to repeat: `compute_ctl` joins a promotion already running).

use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::Op;
use crate::http::{DEFAULT_TIMEOUT, Http};
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

/// How long `configure` waits: `compute_ctl` answers once the spec is
/// applied (a cold start includes the basebackup).
pub const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(300);
/// How long `terminate` waits: `fast` holds the answer 30 s on purpose.
pub const TERMINATE_TIMEOUT: Duration = Duration::from_secs(120);
/// How long `promote` waits: the replica first replays to `wal_flush_lsn`.
pub const PROMOTE_TIMEOUT: Duration = Duration::from_secs(300);

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

/// `LfcPrewarmState`: `status` is `not_prewarmed`, `prewarming`,
/// `completed`, `skipped`, `failed` (with `error`) or `cancelled`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PrewarmState {
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}

impl ComputeCtlClient {
    /// `jwt` is the compute's own token, signed by `pg-control`. A URL with
    /// credentials is refused.
    pub fn new(base: Url, jwt: Secret<String>) -> Result<Self, NeonError> {
        Ok(Self {
            http: Http::new(Component::ComputeCtl, base, Some(jwt))?,
        })
    }

    /// `GET /status`.
    pub async fn status(&self) -> Result<ComputeStatusResponse, NeonError> {
        self.http.get(Op::ComputeStatus, "/status").await
    }

    /// `POST /configure`: hand a waiting compute its spec, or reconfigure a
    /// running one. Waits up to [`CONFIGURE_TIMEOUT`].
    pub async fn configure(
        &self,
        spec: &ComputeSpec,
        config: &ComputeCtlConfig,
    ) -> Result<ComputeStatusResponse, NeonError> {
        let body = ConfigurationRequest {
            spec,
            compute_ctl_config: config,
        };
        let req = self
            .http
            .request(Method::POST, "/configure", CONFIGURE_TIMEOUT)
            .json(&body);
        self.http.send(Op::Configure, req).await
    }

    /// `POST /terminate?mode=`: the LSN Postgres stopped at, when it knows
    /// it. Waits up to [`TERMINATE_TIMEOUT`].
    pub async fn terminate(&self, mode: TerminateMode) -> Result<Option<Lsn>, NeonError> {
        let req = self
            .http
            .request(Method::POST, "/terminate", TERMINATE_TIMEOUT)
            .query(&[("mode", mode)]);
        let resp: TerminateResponse = self.http.send(Op::Terminate, req).await?;
        Ok(resp.lsn)
    }

    /// `POST /promote`: a replica becomes the primary, with `spec` (in
    /// `Primary` mode) once its WAL reaches `wal_flush_lsn`. Waits up to
    /// [`PROMOTE_TIMEOUT`].
    ///
    /// `Ok` only when `compute_ctl` answers `completed`. `compute_ctl`
    /// refuses a replica whose local file cache is not prewarmed ("compute
    /// NotPrewarmed"), so a failover target is started with `autoprewarm`
    /// and endpoint storage, or prewarmed with [`Self::prewarm`] first
    /// (R2.13). A refusal or a failed promotion is `failed_precondition`:
    /// replace the replica (or prewarm it and retry).
    pub async fn promote(&self, spec: &ComputeSpec, wal_flush_lsn: Lsn) -> Result<(), NeonError> {
        let op = Op::Promote;
        let body = PromoteConfig {
            spec,
            wal_flush_lsn,
        };
        let req = self
            .http
            .request(Method::POST, "/promote", PROMOTE_TIMEOUT)
            .json(&body);
        let state: PromoteState = self.http.send(op, req).await?;
        if state.status == "completed" {
            Ok(())
        } else {
            Err(NeonError {
                status: 200,
                msg: format!("promotion answered {:?}", state.status),
                component: Component::ComputeCtl,
                op,
            })
        }
    }

    /// `POST /lfc/prewarm[?from_endpoint=]`: start filling the local file
    /// cache from the state endpoint storage holds (this endpoint's, or
    /// `from_endpoint`'s). Accepted (202); a second request while one runs
    /// is 429 (`aborted`). Follow it with [`Self::prewarm_state`].
    pub async fn prewarm(&self, from_endpoint: Option<&str>) -> Result<(), NeonError> {
        let mut req = self
            .http
            .request(Method::POST, "/lfc/prewarm", DEFAULT_TIMEOUT);
        if let Some(ep) = from_endpoint {
            req = req.query(&[("from_endpoint", ep)]);
        }
        let _: serde_json::Value = self.http.send(Op::Prewarm, req).await?;
        Ok(())
    }

    /// `GET /lfc/prewarm`.
    pub async fn prewarm_state(&self) -> Result<PrewarmState, NeonError> {
        self.http.get(Op::PrewarmState, "/lfc/prewarm").await
    }
}
