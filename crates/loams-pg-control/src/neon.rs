//! The seam between `pg-control` and Neon's components (PG2 Task 5).
//!
//! - [`NeonRead`] is what the API service reads: a timeline's view, the LSN
//!   of a time (leased, so GC keeps it until the branch is created) and the
//!   WAL heads from `loams-wal`.
//! - [`NeonWrite`] is what Task 7's reconcilers change: tenants and
//!   timelines on the pageserver (or the storage controller) and on
//!   `loams-wal`.
//! - [`NeonClients`] implements both over `loams-postgres`'s `NeonClient`
//!   and `WalClient`. It lives here, not in `loams-postgres`, so that the
//!   client crate never depends on `pg-control`. Tests use fakes.
//!
//! Ids and LSNs are `loams-postgres`'s typed ones; a failure is already a
//! registered [`Reason`] with the component it came from.

use std::future::Future;

pub use loams_postgres::pageserver::{NeonClient, TenantConfig, TimelineCreate};
pub use loams_postgres::wal::{WalClient, WalTimelineCreate};
pub use loams_postgres::{Component, Lsn, NeonError, TenantId, TimelineId};
use serde::{Deserialize, Serialize};

use crate::service::Reason;

/// A branch's WAL heads, from `loams-wal` (`WalClient::timeline_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalHeads {
    pub commit_lsn: Lsn,
    pub flush_lsn: Lsn,
    pub remote_consistent_lsn: Lsn,
    pub backup_lsn: Lsn,
}

/// What the pageserver says of a timeline (`TimelineInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineView {
    /// What the pageserver has ingested; `loams-wal`'s `commit_lsn` can be
    /// ahead of it.
    pub last_record_lsn: Lsn,
    /// The oldest LSN a branch or a read may start at: below it, the
    /// history is gone (`lsn_out_of_retention`).
    pub min_readable_lsn: Lsn,
    pub logical_size_bytes: u64,
}

/// The LSN of a time (`get_lsn_by_timestamp`'s `kind`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LsnAtTime {
    /// The time falls in the timeline's WAL: the LSN of the last commit at
    /// or before it.
    Present(Lsn),
    /// After the last commit the pageserver has: the head.
    Future(Lsn),
    /// Before the oldest WAL the pageserver keeps.
    Past,
    /// The timeline has no commit to judge the time by.
    NoData,
}

/// A failed call to a Neon component, mapped to a registered reason
/// (`NeonError::reason()`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {message}", reason.as_str())]
pub struct NeonApiError {
    pub reason: Reason,
    pub component: Option<Component>,
    /// Not for the caller: it can name internal hosts.
    pub message: String,
}

impl From<NeonError> for NeonApiError {
    fn from(e: NeonError) -> Self {
        NeonApiError {
            reason: Reason::from_name(e.reason()),
            component: Some(e.component),
            message: e.msg,
        }
    }
}

/// What `pg-control`'s API service reads from Neon's components.
pub trait NeonRead: Send + Sync + 'static {
    /// The pageserver's view of a timeline.
    fn timeline(
        &self,
        tenant: TenantId,
        timeline: TimelineId,
    ) -> impl Future<Output = Result<TimelineView, NeonApiError>> + Send;

    /// The LSN of the time `at_ms` (ms since the Unix epoch) on a timeline,
    /// leased by the pageserver so GC keeps it while the branch is created.
    fn lsn_by_timestamp(
        &self,
        tenant: TenantId,
        timeline: TimelineId,
        at_ms: u64,
    ) -> impl Future<Output = Result<LsnAtTime, NeonApiError>> + Send;

    /// The timeline's WAL heads, from `loams-wal`.
    fn wal_heads(
        &self,
        tenant: TenantId,
        timeline: TimelineId,
    ) -> impl Future<Output = Result<WalHeads, NeonApiError>> + Send;
}

/// What the reconcilers (Task 7) change. Every call is idempotent the way
/// Neon makes it: a 409 or an existing timeline is the caller's to read as
/// done.
pub trait NeonWrite: NeonRead {
    /// Attaches (or creates) the tenant.
    fn attach_tenant(
        &self,
        tenant: TenantId,
        generation: u32,
        config: &TenantConfig,
    ) -> impl Future<Output = Result<(), NeonApiError>> + Send;

    /// Sets the tenant's configuration (`pitr_interval`, ...).
    fn tenant_config(
        &self,
        tenant: TenantId,
        config: &TenantConfig,
    ) -> impl Future<Output = Result<(), NeonApiError>> + Send;

    /// Creates a timeline on the pageserver: bootstrapped, or a branch.
    fn create_timeline(
        &self,
        tenant: TenantId,
        create: &TimelineCreate,
    ) -> impl Future<Output = Result<TimelineView, NeonApiError>> + Send;

    /// Deletes a timeline from the pageserver.
    fn delete_timeline(
        &self,
        tenant: TenantId,
        timeline: TimelineId,
    ) -> impl Future<Output = Result<(), NeonApiError>> + Send;

    /// Creates the timeline on `loams-wal` (idempotent by id, R2.10).
    fn wal_create_timeline(
        &self,
        create: &WalTimelineCreate,
    ) -> impl Future<Output = Result<WalHeads, NeonApiError>> + Send;
}

/// [`NeonRead`] and [`NeonWrite`] over `loams-postgres`'s clients.
#[derive(Debug, Clone)]
pub struct NeonClients {
    /// The pageserver, or the storage controller.
    pub storage: NeonClient,
    pub wal: WalClient,
}

fn view(info: &loams_postgres::pageserver::TimelineInfo) -> TimelineView {
    TimelineView {
        last_record_lsn: info.last_record_lsn,
        min_readable_lsn: info.min_readable_lsn,
        logical_size_bytes: info.current_logical_size,
    }
}

fn heads(status: &loams_postgres::wal::WalTimelineStatus) -> WalHeads {
    WalHeads {
        commit_lsn: status.commit_lsn,
        flush_lsn: status.flush_lsn,
        remote_consistent_lsn: status.remote_consistent_lsn,
        backup_lsn: status.backup_lsn,
    }
}

impl NeonRead for NeonClients {
    async fn timeline(
        &self,
        tenant: TenantId,
        timeline: TimelineId,
    ) -> Result<TimelineView, NeonApiError> {
        Ok(view(&self.storage.timeline(tenant, timeline).await?))
    }

    async fn lsn_by_timestamp(
        &self,
        tenant: TenantId,
        timeline: TimelineId,
        at_ms: u64,
    ) -> Result<LsnAtTime, NeonApiError> {
        let nanos = i128::from(at_ms) * 1_000_000;
        let at =
            time::OffsetDateTime::from_unix_timestamp_nanos(nanos).map_err(|e| NeonApiError {
                reason: Reason::InvalidArgument,
                component: None,
                message: format!("a time out of range: {e}"),
            })?;
        let found = self
            .storage
            .lsn_by_timestamp(tenant, timeline, at, true)
            .await?;
        match found.kind.as_str() {
            "present" => Ok(LsnAtTime::Present(found.lsn)),
            "future" => Ok(LsnAtTime::Future(found.lsn)),
            "past" => Ok(LsnAtTime::Past),
            "nodata" => Ok(LsnAtTime::NoData),
            other => Err(NeonApiError {
                reason: Reason::Internal,
                component: Some(Component::Pageserver),
                message: format!("an unknown get_lsn_by_timestamp kind {other:?}"),
            }),
        }
    }

    async fn wal_heads(
        &self,
        tenant: TenantId,
        timeline: TimelineId,
    ) -> Result<WalHeads, NeonApiError> {
        Ok(heads(&self.wal.timeline_status(tenant, timeline).await?))
    }
}

impl NeonWrite for NeonClients {
    async fn attach_tenant(
        &self,
        tenant: TenantId,
        generation: u32,
        config: &TenantConfig,
    ) -> Result<(), NeonApiError> {
        Ok(self
            .storage
            .attach_tenant(tenant, generation, config)
            .await?)
    }

    async fn tenant_config(
        &self,
        tenant: TenantId,
        config: &TenantConfig,
    ) -> Result<(), NeonApiError> {
        Ok(self.storage.tenant_config(tenant, config).await?)
    }

    async fn create_timeline(
        &self,
        tenant: TenantId,
        create: &TimelineCreate,
    ) -> Result<TimelineView, NeonApiError> {
        Ok(view(&self.storage.create_timeline(tenant, create).await?))
    }

    async fn delete_timeline(
        &self,
        tenant: TenantId,
        timeline: TimelineId,
    ) -> Result<(), NeonApiError> {
        Ok(self.storage.delete_timeline(tenant, timeline).await?)
    }

    async fn wal_create_timeline(
        &self,
        create: &WalTimelineCreate,
    ) -> Result<WalHeads, NeonApiError> {
        Ok(heads(&self.wal.create_timeline(create).await?))
    }
}
