//! `loams-wal`'s timeline API (`crates/loams-safekeeper/src/http.rs`). The
//! contract is `loams-wal`'s alone (PG2 ruling R2.11): stock safekeepers are
//! gone, so nothing here promises to speak to one.

use reqwest::Method;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::Op;
use crate::http::Http;
use crate::{Component, Lsn, NeonError, Secret, TenantId, TimelineId};

/// A client of one `loams-wal` (an acceptor or a pool's Service).
#[derive(Clone, Debug)]
pub struct WalClient {
    http: Http,
}

/// `POST /v1/tenant/timeline`'s body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WalTimelineCreate {
    pub tenant_id: TenantId,
    pub timeline_id: TimelineId,
    /// A major (17) or a full version (170005).
    pub pg_version: u32,
    /// Where the timeline's WAL starts.
    pub start_lsn: Lsn,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wal_seg_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_lsn: Option<Lsn>,
}

/// A timeline's head (`TimelineStatus`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WalTimelineStatus {
    pub tenant_id: TenantId,
    pub timeline_id: TimelineId,
    pub term: u64,
    pub last_log_term: u64,
    pub flush_lsn: Lsn,
    pub commit_lsn: Lsn,
    pub backup_lsn: Lsn,
    pub remote_consistent_lsn: Lsn,
    pub peer_horizon_lsn: Lsn,
    pub timeline_start_lsn: Lsn,
    pub trimmed_lsn: Lsn,
    pub pg_version: u32,
    pub system_id: u64,
    pub wal_seg_size: u32,
}

impl WalClient {
    /// `auth` is `loams-wal`'s `--auth-token`, sent as a bearer token. A URL
    /// with credentials is refused.
    pub fn new(base: Url, auth: Option<Secret<String>>) -> Result<Self, NeonError> {
        Ok(Self {
            http: Http::new(Component::Wal, base, auth)?,
        })
    }

    /// `POST /v1/tenant/timeline`: create the timeline, or answer the
    /// existing one's head unchanged (200), even when the request's
    /// parameters differ (R2.10). A caller that cares compares the answer
    /// with what it asked for.
    pub async fn create_timeline(
        &self,
        create: &WalTimelineCreate,
    ) -> Result<WalTimelineStatus, NeonError> {
        self.http
            .json(
                Op::WalCreateTimeline,
                Method::POST,
                "/v1/tenant/timeline",
                create,
            )
            .await
    }

    /// `GET /v1/tenant/{t}/timeline/{tl}`.
    pub async fn timeline_status(
        &self,
        t: TenantId,
        tl: TimelineId,
    ) -> Result<WalTimelineStatus, NeonError> {
        self.http
            .get(
                Op::WalTimelineStatus,
                &format!("/v1/tenant/{t}/timeline/{tl}"),
            )
            .await
    }
}
