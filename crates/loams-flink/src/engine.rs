//! The `ManagedEngine`-shaped trait the Flink controller implements (D1976).
//!
//! ENG-UI owns the shared trait in `crates/loams-engines`; until it is on
//! `dev` this is a local copy of the shape RW-a proposed in engines-81.md,
//! so the switch is a type rename: `kind`, `create`, `scale`, `upgrade`,
//! `suspend`, `resume`, `delete`, `status`, `health`, [`EngineStatus`],
//! [`EngineError`] and the [`MeteringHook`].

use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::{Resources, Size};
use crate::storage::TenantId;

/// Which engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    /// RisingWave.
    RisingWave,
    /// Flink SQL.
    Flink,
    /// Spark.
    Spark,
}

/// A cluster's lifecycle phase (the shape RW-a proposed).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "phase")]
pub enum Phase {
    /// Being created.
    Provisioning,
    /// Up.
    Running,
    /// Changing size.
    Scaling,
    /// Moving to another version.
    Upgrading,
    /// Stopping jobs with savepoints, then scaling to zero.
    Suspending,
    /// Scaled to zero; state kept.
    Suspended,
    /// Scaling up and restoring jobs.
    Resuming,
    /// Being removed.
    Deleting,
    /// Removed.
    Deleted,
    /// `op` failed; retry it or delete.
    Failed {
        /// The operation that failed.
        op: String,
    },
}

impl Phase {
    /// Whether an operation is in flight (a second one is refused).
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Phase::Provisioning
                | Phase::Scaling
                | Phase::Upgrading
                | Phase::Suspending
                | Phase::Resuming
                | Phase::Deleting
        )
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Phase::Provisioning => f.write_str("provisioning"),
            Phase::Running => f.write_str("running"),
            Phase::Scaling => f.write_str("scaling"),
            Phase::Upgrading => f.write_str("upgrading"),
            Phase::Suspending => f.write_str("suspending"),
            Phase::Suspended => f.write_str("suspended"),
            Phase::Resuming => f.write_str("resuming"),
            Phase::Deleting => f.write_str("deleting"),
            Phase::Deleted => f.write_str("deleted"),
            Phase::Failed { op } => write!(f, "failed ({op})"),
        }
    }
}

/// A version to move to: the Flink version and the image that carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineVersion {
    /// `MAJOR.MINOR.PATCH`.
    pub version: String,
    /// The image, pinned by digest.
    pub image: String,
}

/// An address a client reaches the cluster on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// `sql-gateway` or `rest`.
    pub name: String,
    /// The host (inside the platform's network).
    pub host: String,
    /// The port.
    pub port: u16,
}

/// What `status` and every lifecycle call return.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineStatus {
    /// The tenant.
    pub tenant: TenantId,
    /// The engine.
    pub kind: EngineKind,
    /// The phase.
    pub phase: Phase,
    /// The running version.
    pub version: String,
    /// The size.
    pub size: Size,
    /// The resources.
    pub resources: Resources,
    /// Where clients reach it (through loams-gateway only).
    pub endpoints: Vec<Endpoint>,
    /// What happened last, for people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A health probe's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// Whether every role answers and enough TaskManagers registered.
    pub healthy: bool,
    /// What was checked and found.
    pub detail: String,
}

/// Why a lifecycle call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// No cluster for the tenant.
    #[error("no Flink cluster for tenant {0}")]
    NotFound(String),
    /// The tenant already has one.
    #[error("tenant {0} already has a Flink cluster")]
    AlreadyExists(String),
    /// Another operation is in flight, or the phase does not allow this.
    #[error("the cluster is {phase}")]
    Conflict {
        /// The phase it is in.
        phase: Phase,
    },
    /// A bad argument.
    #[error("invalid: {0}")]
    Invalid(String),
    /// Credentials, buckets or databases could not be provisioned.
    #[error("provisioning failed: {0}")]
    Provision(String),
    /// The platform or Flink failed.
    #[error("runtime: {0}")]
    Runtime(String),
}

/// Called after every phase change (ENG-UI owns the real one: Loams Meter).
#[async_trait]
pub trait MeteringHook: Send + Sync {
    /// The cluster is now `status`.
    async fn on_change(&self, status: &EngineStatus);
}

/// A hook that does nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoMetering;

#[async_trait]
impl MeteringHook for NoMetering {
    async fn on_change(&self, _status: &EngineStatus) {}
}

/// A managed engine's lifecycle.
#[async_trait]
pub trait ManagedEngine: Send + Sync {
    /// What creates a cluster.
    type Spec: Clone + Send + Sync;

    /// Which engine this is.
    fn kind(&self) -> EngineKind;

    /// Creates the tenant's cluster.
    async fn create(
        &self,
        tenant: &TenantId,
        spec: Self::Spec,
    ) -> Result<EngineStatus, EngineError>;

    /// Changes its size (`overrides` replaces the size's resources).
    async fn scale(
        &self,
        tenant: &TenantId,
        size: Size,
        overrides: Option<Resources>,
    ) -> Result<EngineStatus, EngineError>;

    /// Moves it to `version`.
    async fn upgrade(
        &self,
        tenant: &TenantId,
        version: EngineVersion,
    ) -> Result<EngineStatus, EngineError>;

    /// Stops it, keeping its state.
    async fn suspend(&self, tenant: &TenantId) -> Result<EngineStatus, EngineError>;

    /// Starts a suspended cluster again.
    async fn resume(&self, tenant: &TenantId) -> Result<EngineStatus, EngineError>;

    /// Removes it; with `purge`, its state too.
    async fn delete(&self, tenant: &TenantId, purge: bool) -> Result<EngineStatus, EngineError>;

    /// Its status.
    async fn status(&self, tenant: &TenantId) -> Result<EngineStatus, EngineError>;

    /// Probes it.
    async fn health(&self, tenant: &TenantId) -> Result<Health, EngineError>;
}

/// `MAJOR.MINOR.PATCH` as numbers.
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Checks an upgrade from `from` to `to`: forward only, at most one major
/// version at a time (savepoints are CANONICAL, which Flink restores across
/// versions; D1975, D1976).
///
/// # Errors
///
/// [`EngineError::Invalid`] saying why.
pub fn check_upgrade(from: &str, to: &str) -> Result<(), EngineError> {
    let parse = |v: &str| {
        parse_version(v)
            .ok_or_else(|| EngineError::Invalid(format!("version {v:?} is not MAJOR.MINOR.PATCH")))
    };
    let (f, t) = (parse(from)?, parse(to)?);
    if t <= f {
        return Err(EngineError::Invalid(format!(
            "upgrades go forward only: {from} to {to}"
        )));
    }
    if t.0 > f.0 + 1 {
        return Err(EngineError::Invalid(format!(
            "one major version at a time: {from} to {to}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_go_forward_one_major_at_a_time() {
        assert!(check_upgrade("2.2.1", "2.3.0").is_ok());
        assert!(check_upgrade("2.2.1", "3.0.0").is_ok());
        assert!(check_upgrade("2.2.1", "2.2.1").is_err());
        assert!(check_upgrade("2.3.0", "2.2.1").is_err());
        assert!(check_upgrade("2.2.1", "4.0.0").is_err());
        assert!(check_upgrade("2.2", "2.3.0").is_err());
    }

    #[test]
    fn busy_phases_refuse_a_second_op() {
        assert!(Phase::Upgrading.is_busy());
        assert!(!Phase::Running.is_busy());
        assert!(
            !Phase::Failed {
                op: "upgrade".into()
            }
            .is_busy()
        );
        assert_eq!(
            Phase::Failed { op: "scale".into() }.to_string(),
            "failed (scale)"
        );
    }
}
