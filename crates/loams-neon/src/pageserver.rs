//! The pageserver's management API, or the same through the storage
//! controller. The request and response shapes are the fork's
//! `libs/pageserver_api/src/models.rs` (`LocationConfig`,
//! `TimelineCreateRequest`, `TenantConfigRequest`, `TimelineInfo`) and
//! `pageserver/src/http/routes.rs`, checked against the pinned image in
//! `tests/fixtures/` (PG2 Task 2).

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize, Serializer};
use url::Url;

use crate::http::Http;
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
/// any other goes in `extra` under its Neon name.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TenantConfig {
    /// How far back a branch can be made or a timeline restored.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "humantime_opt"
    )]
    pub pitr_interval: Option<Duration>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn humantime_opt<S: Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
    match d {
        Some(d) => s.collect_str(&humantime::format_duration(*d)),
        None => s.serialize_none(),
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

/// `TimelineInfo`, the fields Loams reads (the rest are ignored).
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
/// "future", "past" or "nodata".
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct LsnByTimestamp {
    pub lsn: Lsn,
    pub kind: String,
}

/// `LocationConfig` for an attached tenant.
#[derive(Serialize)]
struct LocationConfig<'a> {
    mode: &'static str,
    generation: u32,
    tenant_conf: &'a TenantConfig,
}

/// `TenantConfigRequest`.
#[derive(Serialize)]
struct TenantConfigRequest<'a> {
    tenant_id: &'a TenantId,
    #[serde(flatten)]
    config: &'a TenantConfig,
}

impl NeonClient {
    /// `auth` is the storage token (a pageserver or controller JWT), sent as
    /// a bearer token when set.
    pub fn new(endpoints: NeonEndpoints, auth: Option<Secret<String>>) -> Self {
        let (component, base) = match endpoints.storcon {
            Some(url) => (Component::StorageController, url),
            None => (Component::Pageserver, endpoints.pageserver),
        };
        let storcon = component == Component::StorageController;
        Self {
            http: Http::new(component, base, auth),
            storcon,
        }
    }

    /// Attach (create) a tenant: `PUT /v1/tenant/{t}/location_config` in
    /// `AttachedSingle` mode at `generation` on a pageserver; through the
    /// storage controller, `POST /v1/tenant`, which picks the generation
    /// itself.
    pub async fn attach_tenant(
        &self,
        t: TenantId,
        generation: u32,
        conf: &TenantConfig,
    ) -> Result<(), NeonError> {
        if self.storcon {
            let body = TenantCreateRequest {
                new_tenant_id: &t,
                config: conf,
            };
            let _: serde_json::Value = self.http.json(Method::POST, "/v1/tenant", &body).await?;
        } else {
            let body = LocationConfig {
                mode: "AttachedSingle",
                generation,
                tenant_conf: conf,
            };
            let _: serde_json::Value = self
                .http
                .json(
                    Method::PUT,
                    &format!("/v1/tenant/{t}/location_config"),
                    &body,
                )
                .await?;
        }
        Ok(())
    }

    /// `POST /v1/tenant/{t}/timeline`.
    pub async fn create_timeline(
        &self,
        t: TenantId,
        create: &TimelineCreate,
    ) -> Result<TimelineInfo, NeonError> {
        self.http
            .json(Method::POST, &format!("/v1/tenant/{t}/timeline"), create)
            .await
    }

    /// `GET /v1/tenant/{t}/timeline`.
    pub async fn list_timelines(&self, t: TenantId) -> Result<Vec<TimelineInfo>, NeonError> {
        self.http.get(&format!("/v1/tenant/{t}/timeline")).await
    }

    /// `GET /v1/tenant/{t}/timeline/{tl}`.
    pub async fn timeline(&self, t: TenantId, tl: TimelineId) -> Result<TimelineInfo, NeonError> {
        self.http
            .get(&format!("/v1/tenant/{t}/timeline/{tl}"))
            .await
    }

    /// `DELETE /v1/tenant/{t}/timeline/{tl}`: accepted (202); the deletion
    /// finishes in the background, and a repeat is `not_found` once it has.
    pub async fn delete_timeline(&self, t: TenantId, tl: TimelineId) -> Result<(), NeonError> {
        let req = self
            .http
            .request(Method::DELETE, &format!("/v1/tenant/{t}/timeline/{tl}"))?;
        let _: serde_json::Value = self.http.send(req).await?;
        Ok(())
    }

    /// `GET .../get_lsn_by_timestamp?timestamp=<RFC 3339>`: the LSN of a
    /// time, for branching or restoring at a time.
    pub async fn lsn_by_timestamp(
        &self,
        t: TenantId,
        tl: TimelineId,
        at: time::OffsetDateTime,
    ) -> Result<LsnByTimestamp, NeonError> {
        let ts = at
            .to_offset(time::UtcOffset::UTC)
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| NeonError::transport(self.http.component, e))?;
        let req = self
            .http
            .request(
                Method::GET,
                &format!("/v1/tenant/{t}/timeline/{tl}/get_lsn_by_timestamp"),
            )?
            .query(&[("timestamp", ts)]);
        self.http.send(req).await
    }

    /// `PUT /v1/tenant/config`: replaces the tenant's settings.
    pub async fn tenant_config(&self, t: TenantId, conf: &TenantConfig) -> Result<(), NeonError> {
        let body = TenantConfigRequest {
            tenant_id: &t,
            config: conf,
        };
        let _: serde_json::Value = self
            .http
            .json(Method::PUT, "/v1/tenant/config", &body)
            .await?;
        Ok(())
    }
}
