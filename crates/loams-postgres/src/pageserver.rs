//! The pageserver's management API, or the same through the storage
//! controller. The request and response shapes are the fork's
//! `libs/pageserver_api/src/models.rs` (`LocationConfig`,
//! `TimelineCreateRequest`, `TenantConfigPatchRequest`, `TimelineInfo`) and
//! `pageserver/src/http/routes.rs`, checked against the pinned image in
//! `tests/fixtures/` (PG2 Task 2).

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize, Serializer};
use url::Url;

use crate::error::Op;
use crate::http::{DEFAULT_TIMEOUT, Http};
use crate::storcon::TenantCreateRequest;
use crate::{Component, Lsn, NeonError, Secret, TenantId, TimelineId};

/// Where Neon's storage is reached.
#[derive(Clone, Debug)]
pub struct NeonEndpoints {
    /// A pageserver's management API (`http://host:9898`).
    pub pageserver: Url,
    /// The storage controller (`http://host:1234`). When set, every call goes
    /// to it: it routes to the tenant's pageservers, chooses generations and
    /// proxies reads (`storage_controller/src/http.rs`).
    pub storcon: Option<Url>,
}

/// A client of the pageserver, or of the storage controller when one is set.
#[derive(Clone, Debug)]
pub struct NeonClient {
    http: Http,
    storcon: bool,
}

/// A tenant's settings (`TenantConfig`); only the ones Loams sets are typed,
/// any other goes in `extra` under its Neon name. A typed setting wins over
/// the same name in `extra`, so each name is sent once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TenantConfig {
    /// How far back a branch can be made or a timeline restored.
    pub pitr_interval: Option<Duration>,
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Serialize for TenantConfig {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map: BTreeMap<&str, serde_json::Value> = self
            .extra
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        if let Some(d) = self.pitr_interval {
            map.insert(
                "pitr_interval",
                humantime::format_duration(d).to_string().into(),
            );
        }
        map.serialize(s)
    }
}

/// A new timeline: bootstrapped from initdb, or a branch of another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineCreate {
    pub new_timeline_id: TimelineId,
    pub ancestor: Option<TimelineId>,
    /// Where in the ancestor; `None` takes its head.
    pub ancestor_start_lsn: Option<Lsn>,
    /// The Postgres major version of a bootstrapped timeline; a branch
    /// inherits its ancestor's.
    pub pg_version: Option<u32>,
}

impl TimelineCreate {
    pub fn bootstrap(new_timeline_id: TimelineId, pg_version: u32) -> Self {
        Self {
            new_timeline_id,
            ancestor: None,
            ancestor_start_lsn: None,
            pg_version: Some(pg_version),
        }
    }

    pub fn branch(new_timeline_id: TimelineId, ancestor: TimelineId, at: Option<Lsn>) -> Self {
        Self {
            new_timeline_id,
            ancestor: Some(ancestor),
            ancestor_start_lsn: at,
            pg_version: None,
        }
    }
}

/// `TimelineCreateRequest`: `new_timeline_id` and a flattened, untagged
/// mode (`Branch{ancestor_timeline_id, ancestor_start_lsn?, pg_version?}` or
/// `Bootstrap{pg_version?}`).
impl Serialize for TimelineCreate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            new_timeline_id: &'a TimelineId,
            #[serde(skip_serializing_if = "Option::is_none")]
            ancestor_timeline_id: Option<&'a TimelineId>,
            #[serde(skip_serializing_if = "Option::is_none")]
            ancestor_start_lsn: Option<&'a Lsn>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pg_version: Option<u32>,
        }
        Wire {
            new_timeline_id: &self.new_timeline_id,
            ancestor_timeline_id: self.ancestor.as_ref(),
            ancestor_start_lsn: self.ancestor.and(self.ancestor_start_lsn.as_ref()),
            pg_version: self.pg_version,
        }
        .serialize(s)
    }
}

/// `TimelineInfo`, the fields Loams reads (the rest are ignored). The
/// storage controller's create answers the same flattened with
/// `safekeepers` (`TimelineCreateResponseStorcon`), which Loams does not use:
/// `loams-wal` is configured by Loams, not by the controller.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TimelineInfo {
    pub tenant_id: TenantId,
    pub timeline_id: TimelineId,
    pub ancestor_timeline_id: Option<TimelineId>,
    pub ancestor_lsn: Option<Lsn>,
    pub last_record_lsn: Lsn,
    pub prev_record_lsn: Option<Lsn>,
    pub disk_consistent_lsn: Lsn,
    pub remote_consistent_lsn: Lsn,
    pub initdb_lsn: Lsn,
    pub min_readable_lsn: Lsn,
    pub applied_gc_cutoff_lsn: Lsn,
    pub current_logical_size: u64,
    pub pg_version: u32,
    /// "Active", "Loading", "Stopping", ... (or an object for "Broken").
    pub state: serde_json::Value,
    #[serde(default)]
    pub is_archived: Option<bool>,
}

/// The LSN for a time (`get_lsn_by_timestamp`): `kind` is "present",
/// "future", "past" or "nodata". With `with_lease`, `valid_until` is when
/// the pageserver's lease on `lsn` ends (RFC 3339, ms), when it granted one
/// (`pageserver/src/http/routes.rs`, `get_lsn_by_timestamp_handler`, the
/// flattened `LsnLease`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct LsnByTimestamp {
    pub lsn: Lsn,
    pub kind: String,
    #[serde(default)]
    pub valid_until: Option<String>,
}

/// `LocationConfig` for an attached tenant.
#[derive(Serialize)]
struct LocationConfig<'a> {
    mode: &'static str,
    generation: u32,
    tenant_conf: &'a TenantConfig,
}

/// `TenantConfigPatchRequest`: a name set to a value is upserted, a name
/// left out is kept.
#[derive(Serialize)]
struct TenantConfigPatchRequest<'a> {
    tenant_id: &'a TenantId,
    #[serde(flatten)]
    config: &'a TenantConfig,
}

impl NeonClient {
    /// `auth` is the storage token (a pageserver or controller JWT), sent as
    /// a bearer token when set. A URL with credentials, a query or a scheme
    /// other than http(s) is refused (`Op::Setup`, reason `internal`).
    pub fn new(endpoints: NeonEndpoints, auth: Option<Secret<String>>) -> Result<Self, NeonError> {
        let (component, base) = match endpoints.storcon {
            Some(url) => (Component::StorageController, url),
            None => (Component::Pageserver, endpoints.pageserver),
        };
        Ok(Self {
            http: Http::new(component, base, auth)?,
            storcon: component == Component::StorageController,
        })
    }

    /// Attach (create) a tenant.
    ///
    /// On a pageserver: `PUT /v1/tenant/{t}/location_config` in
    /// `AttachedSingle` mode at `generation`, with `conf` as its whole
    /// config. Through the storage controller: `POST /v1/tenant`
    /// (`TenantCreateRequest`) for an unsharded tenant with the controller's
    /// default placement (attached, no secondary). The controller owns
    /// generations, so `generation` is not sent; placement policies and
    /// shards are not supported yet (R2.14: storage HA, Task 49).
    pub async fn attach_tenant(
        &self,
        t: TenantId,
        generation: u32,
        conf: &TenantConfig,
    ) -> Result<(), NeonError> {
        let op = Op::AttachTenant;
        if self.storcon {
            let body = TenantCreateRequest {
                new_tenant_id: &t,
                config: conf,
            };
            let _: serde_json::Value = self
                .http
                .json(op, Method::POST, "/v1/tenant", &body)
                .await?;
        } else {
            let body = LocationConfig {
                mode: "AttachedSingle",
                generation,
                tenant_conf: conf,
            };
            let path = format!("/v1/tenant/{t}/location_config");
            let _: serde_json::Value = self.http.json(op, Method::PUT, &path, &body).await?;
        }
        Ok(())
    }

    /// `POST /v1/tenant/{t}/timeline`. The same id with the same parameters
    /// answers the existing timeline; with other parameters it is a 409.
    pub async fn create_timeline(
        &self,
        t: TenantId,
        create: &TimelineCreate,
    ) -> Result<TimelineInfo, NeonError> {
        let path = format!("/v1/tenant/{t}/timeline");
        self.http
            .json(Op::CreateTimeline, Method::POST, &path, create)
            .await
    }

    /// `GET /v1/tenant/{t}/timeline`.
    pub async fn list_timelines(&self, t: TenantId) -> Result<Vec<TimelineInfo>, NeonError> {
        self.http
            .get(Op::ListTimelines, &format!("/v1/tenant/{t}/timeline"))
            .await
    }

    /// `GET /v1/tenant/{t}/timeline/{tl}`.
    pub async fn timeline(&self, t: TenantId, tl: TimelineId) -> Result<TimelineInfo, NeonError> {
        self.http
            .get(Op::GetTimeline, &format!("/v1/tenant/{t}/timeline/{tl}"))
            .await
    }

    /// `DELETE /v1/tenant/{t}/timeline/{tl}`. `Ok` means accepted, not done.
    ///
    /// A pageserver answers 202 and finishes in the background; a repeat is
    /// 404 (`not_found`) once it has, 409 (`aborted`) while it runs, and 412
    /// for a timeline with children (`branch_has_children`) or a missing
    /// tenant (`not_found`). The storage controller retries for up to 25 s
    /// and answers 200 once the timeline is gone, or 409 (`aborted`) if it is
    /// still going. Either way the caller polls [`Self::timeline`] until it
    /// is `not_found`.
    pub async fn delete_timeline(&self, t: TenantId, tl: TimelineId) -> Result<(), NeonError> {
        let path = format!("/v1/tenant/{t}/timeline/{tl}");
        let req = self.http.request(Method::DELETE, &path, DEFAULT_TIMEOUT);
        let _: serde_json::Value = self.http.send(Op::DeleteTimeline, req).await?;
        Ok(())
    }

    /// `GET .../get_lsn_by_timestamp?timestamp=<RFC 3339>`: the LSN of a
    /// time, for branching or restoring at a time. With `with_lease`, the
    /// pageserver also leases the LSN (`&with_lease=true`), so GC keeps it
    /// until the branch is created.
    pub async fn lsn_by_timestamp(
        &self,
        t: TenantId,
        tl: TimelineId,
        at: time::OffsetDateTime,
        with_lease: bool,
    ) -> Result<LsnByTimestamp, NeonError> {
        let op = Op::LsnByTimestamp;
        let ts = at
            .to_offset(time::UtcOffset::UTC)
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| NeonError {
                status: 400,
                msg: format!("timestamp: {e}"),
                component: self.http.component,
                op,
            })?;
        let path = format!("/v1/tenant/{t}/timeline/{tl}/get_lsn_by_timestamp");
        let req = self
            .http
            .request(Method::GET, &path, DEFAULT_TIMEOUT)
            .query(&[("timestamp", ts)]);
        let req = if with_lease {
            req.query(&[("with_lease", "true")])
        } else {
            req
        };
        self.http.send(op, req).await
    }

    /// `PATCH /v1/tenant/config`: sets the settings in `conf` and keeps every
    /// other (the pageserver's `TenantConfigPatch`).
    pub async fn tenant_config(&self, t: TenantId, conf: &TenantConfig) -> Result<(), NeonError> {
        let body = TenantConfigPatchRequest {
            tenant_id: &t,
            config: conf,
        };
        let _: serde_json::Value = self
            .http
            .json(Op::TenantConfig, Method::PATCH, "/v1/tenant/config", &body)
            .await?;
        Ok(())
    }
}
