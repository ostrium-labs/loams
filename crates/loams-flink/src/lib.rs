//! Managed Flink SQL for Loams (design 81, plan FL1). Preview: written
//! against the Flink 2.2 REST documentation, not yet run end to end.
//!
//! Apache Flink (Apache-2.0) runs unmodified from its official image; this
//! crate only talks to it over HTTP and renders its configuration.
//!
//! - [`sql_gateway`]: a typed client for the Flink SQL Gateway REST API
//!   (v2, v3 and v4 endpoints): sessions, statements, paged results,
//!   operations, `configure-session`, `complete-statement` and
//!   `deploy-script` (application mode, v4).
//! - [`rest`]: a typed client for the JobManager REST API: jobs, cancel,
//!   stop-with-savepoint, savepoints and checkpoints.
//! - [`storage`]: tenant ids and the per-tenant layout on Loams object
//!   storage (D1973).
//! - [`config`]: the cluster spec and its `config.yaml` (D1972, D1974,
//!   D1975).
//! - [`k8s`]: the Kubernetes objects of one tenant's session cluster.
//! - [`controller`]: the session-cluster lifecycle (create, scale, upgrade
//!   through savepoints, suspend, resume, delete; D1976) over a
//!   [`controller::ClusterRuntime`], and the compose recovery record (D1974).
//! - [`engine`]: the `ManagedEngine`-shaped trait the controller implements
//!   until ENG-UI's `loams-engines` lands.
//! - [`gateway`]: the loams-gateway module that is the only way to the SQL
//!   Gateway, with Loams auth in [`gateway::auth`] (D1977, D1978).

pub mod config;
pub mod controller;
pub mod engine;
pub mod error;
pub mod gateway;
mod http;
pub mod k8s;
pub mod rest;
#[cfg(feature = "kubernetes")]
pub mod runtime_kube;
pub mod sql_gateway;
pub mod storage;

// FL-b's modules (engines-81.md, FL-b): the session bootstrap
// (`catalog`), connector tables and the jar bundle (`connectors`), the
// namespace-scoped HTTP API (`api`) and `loams flink` (`cli`).
pub mod api;
pub mod catalog;
pub mod cli;
pub mod connectors;

pub use error::FlinkError;
pub use rest::FlinkRestClient;
pub use sql_gateway::SqlGatewayClient;
pub use storage::TenantId;

/// The Flink release the defaults target (D1970).
pub const FLINK_VERSION: &str = "2.2.1";

/// The official image the defaults run, pinned by digest (D1970). Flink 2.2.1
/// was released on 2026-05-15; the digest is the multi-arch index Docker Hub
/// served on 2026-10-27.
pub const DEFAULT_IMAGE: &str = "docker.io/library/flink:2.2.1-scala_2.12-java21@sha256:521951efbfbb0cec74c2826446ce1d20a051bff18e6d3716a6affe97ddec692a";
