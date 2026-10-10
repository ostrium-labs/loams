//! The compute spec `compute_ctl` runs a Postgres from (the fork's
//! `libs/compute_api/src/spec.rs` `ComputeSpec`), built from Loams' records
//! (§46 §6–§7). Only the fields Loams sets are written; `compute_ctl`
//! defaults the rest. `tests/fixtures/spec_main.json` is the golden form.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Serialize, Serializer};

use crate::{Lsn, Secret, TenantId, TimelineId};

fn expose<S: Serializer>(v: &Option<Secret<String>>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(secret) => s.serialize_str(secret.expose()),
        None => s.serialize_none(),
    }
}

/// `ComputeMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ComputeMode {
    /// The read-write compute.
    Primary,
    /// Read-only, pinned at an LSN.
    Static(Lsn),
    /// Read-only, following the branch's head (hot standby).
    Replica,
}

/// `GenericOption`: one Postgres setting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Setting {
    pub name: String,
    pub value: Option<String>,
    /// "bool", "integer", "string", "enum", ...
    pub vartype: String,
}

impl Setting {
    pub fn new(name: &str, value: &str, vartype: &str) -> Self {
        Self {
            name: name.to_owned(),
            value: Some(value.to_owned()),
            vartype: vartype.to_owned(),
        }
    }
}

/// A role record.
#[derive(Clone, PartialEq, Eq)]
pub struct RoleRec {
    pub name: String,
    /// The SCRAM verifier (never the password).
    pub encrypted_password: Option<String>,
}

impl fmt::Debug for RoleRec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoleRec")
            .field("name", &self.name)
            .field(
                "encrypted_password",
                &self.encrypted_password.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

/// A database record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseRec {
    pub name: String,
    pub owner: String,
}

/// `Role`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Role {
    pub name: String,
    #[serde(serialize_with = "expose")]
    pub encrypted_password: Option<Secret<String>>,
    pub options: Option<Vec<Setting>>,
}

/// `Database`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Database {
    pub name: String,
    pub owner: String,
    pub options: Option<Vec<Setting>>,
}

/// `Cluster`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Cluster {
    pub cluster_id: Option<String>,
    pub name: Option<String>,
    pub state: Option<String>,
    pub roles: Vec<Role>,
    pub databases: Vec<Database>,
    pub settings: Option<Vec<Setting>>,
}

/// `PageserverShardConnectionInfo`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PageserverShardConnectionInfo {
    pub id: Option<u64>,
    pub libpq_url: Option<String>,
    pub grpc_url: Option<String>,
}

/// `PageserverShardInfo`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PageserverShardInfo {
    pub pageservers: Vec<PageserverShardConnectionInfo>,
}

/// `PageserverConnectionInfo`, for an unsharded tenant (shard index "0000").
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PageserverConnectionInfo {
    pub shard_count: u8,
    pub stripe_size: Option<u32>,
    pub shards: BTreeMap<String, PageserverShardInfo>,
    pub prefer_protocol: String,
}

/// `ComputeSpec`, the fields Loams sets.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ComputeSpec {
    pub format_version: f32,
    /// The compute id: `compute_ctl` reports it back as the operation.
    pub operation_uuid: Option<String>,
    pub suspend_timeout_seconds: i64,
    pub cluster: Cluster,
    pub delta_operations: Option<Vec<serde_json::Value>>,
    pub tenant_id: TenantId,
    pub timeline_id: TimelineId,
    /// The older single-pageserver form, kept beside
    /// `pageserver_connection_info` for `compute_ctl`s that predate it.
    pub pageserver_connstring: Option<String>,
    pub pageserver_connection_info: Option<PageserverConnectionInfo>,
    pub project_id: Option<String>,
    pub branch_id: Option<String>,
    pub endpoint_id: Option<String>,
    /// The `loams-wal` acceptors, or a pool's Service.
    pub safekeeper_connstrings: Vec<String>,
    pub mode: ComputeMode,
    #[serde(serialize_with = "expose")]
    pub storage_auth_token: Option<Secret<String>>,
    /// Endpoint storage (`host:port`), where `compute_ctl` keeps and reads
    /// the local file cache's state; needed to prewarm (R2.13).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_storage_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "expose")]
    pub endpoint_storage_token: Option<Secret<String>>,
    /// Fill the local file cache from endpoint storage at start, so a
    /// replica can be promoted (`compute_ctl` refuses an unprewarmed one).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub autoprewarm: bool,
}

/// Builds a [`ComputeSpec`] from records.
#[derive(Clone, Debug)]
pub struct ComputeSpecBuilder {
    tenant_id: TenantId,
    timeline_id: TimelineId,
    project_id: Option<String>,
    branch_id: Option<String>,
    endpoint_id: Option<String>,
    compute_id: Option<String>,
    safekeepers: Vec<String>,
    pageserver: Option<(u64, String)>,
    storage_auth_token: Option<Secret<String>>,
    roles: Vec<RoleRec>,
    databases: Vec<DatabaseRec>,
    settings: Vec<Setting>,
    max_cluster_size_mb: Option<u64>,
    suspend_timeout_seconds: i64,
    mode: ComputeMode,
    endpoint_storage: Option<(String, Secret<String>)>,
    autoprewarm: bool,
}

impl ComputeSpecBuilder {
    pub fn new(tenant_id: TenantId, timeline_id: TimelineId) -> Self {
        Self {
            tenant_id,
            timeline_id,
            project_id: None,
            branch_id: None,
            endpoint_id: None,
            compute_id: None,
            safekeepers: Vec::new(),
            pageserver: None,
            storage_auth_token: None,
            roles: Vec::new(),
            databases: Vec::new(),
            settings: Vec::new(),
            max_cluster_size_mb: None,
            suspend_timeout_seconds: -1,
            mode: ComputeMode::Primary,
            endpoint_storage: None,
            autoprewarm: false,
        }
    }

    /// Endpoint storage (`host:port`) and its token, for the local file
    /// cache's state.
    pub fn endpoint_storage(mut self, addr: &str, token: Secret<String>) -> Self {
        self.endpoint_storage = Some((addr.to_owned(), token));
        self
    }

    /// Prewarm the local file cache from endpoint storage at start (a
    /// failover target, R2.13). Needs [`Self::endpoint_storage`].
    pub fn autoprewarm(mut self, on: bool) -> Self {
        self.autoprewarm = on;
        self
    }

    /// The Loams ids (`prj-`, `br-`, `ep-`), which `compute_ctl` exposes as
    /// `neon.project_id` and friends.
    pub fn ids(mut self, project: &str, branch: &str, endpoint: &str) -> Self {
        self.project_id = Some(project.to_owned());
        self.branch_id = Some(branch.to_owned());
        self.endpoint_id = Some(endpoint.to_owned());
        self
    }

    /// The compute's id (`cmp-`).
    pub fn compute_id(mut self, id: &str) -> Self {
        self.compute_id = Some(id.to_owned());
        self
    }

    pub fn safekeepers<I: IntoIterator<Item = S>, S: Into<String>>(mut self, connstrs: I) -> Self {
        self.safekeepers = connstrs.into_iter().map(Into::into).collect();
        self
    }

    /// The pageserver's node id and libpq URL.
    pub fn pageserver(mut self, node_id: u64, libpq_url: &str) -> Self {
        self.pageserver = Some((node_id, libpq_url.to_owned()));
        self
    }

    /// The tenant-scoped token the compute reads pages with.
    pub fn storage_auth_token(mut self, token: Secret<String>) -> Self {
        self.storage_auth_token = Some(token);
        self
    }

    pub fn role(mut self, role: RoleRec) -> Self {
        self.roles.push(role);
        self
    }

    pub fn database(mut self, db: DatabaseRec) -> Self {
        self.databases.push(db);
        self
    }

    pub fn setting(mut self, s: Setting) -> Self {
        self.settings.push(s);
        self
    }

    /// `neon.max_cluster_size`, in MB (the project's storage quota, §46 §14).
    pub fn max_cluster_size_mb(mut self, mb: u64) -> Self {
        self.max_cluster_size_mb = Some(mb);
        self
    }

    /// Idle seconds before `compute_ctl` reports the compute suspendable; -1
    /// never.
    pub fn suspend_timeout_seconds(mut self, s: i64) -> Self {
        self.suspend_timeout_seconds = s;
        self
    }

    pub fn mode(mut self, mode: ComputeMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn build(self) -> ComputeSpec {
        // What every Neon compute needs, before the caller's settings: the
        // neon extension (the storage manager and walproposer), walproposer
        // as the synchronous standby of a primary, the backpressure lags the
        // pageserver's feedback drives (R31.1), SCRAM, and no in-place
        // restart (a failed compute is replaced, §46 §3).
        let mut settings = vec![
            Setting::new("shared_preload_libraries", "neon", "string"),
            Setting::new("restart_after_crash", "off", "bool"),
            Setting::new("password_encryption", "scram-sha-256", "enum"),
            Setting::new("max_replication_write_lag", "500MB", "integer"),
            Setting::new("max_replication_flush_lag", "10GB", "integer"),
        ];
        if self.mode == ComputeMode::Primary {
            settings.push(Setting::new(
                "synchronous_standby_names",
                "walproposer",
                "string",
            ));
        }
        // One line per name: a caller's setting replaces a default in place,
        // and the quota replaces a caller's `neon.max_cluster_size`.
        let quota = self
            .max_cluster_size_mb
            .map(|mb| Setting::new("neon.max_cluster_size", &mb.to_string(), "integer"));
        for s in self.settings.into_iter().chain(quota) {
            match settings.iter_mut().find(|d| d.name == s.name) {
                Some(slot) => *slot = s,
                None => settings.push(s),
            }
        }
        let (pageserver_connstring, pageserver_connection_info) = match self.pageserver {
            Some((id, url)) => (
                Some(url.clone()),
                Some(PageserverConnectionInfo {
                    shard_count: 0,
                    stripe_size: None,
                    shards: BTreeMap::from([(
                        "0000".to_owned(),
                        PageserverShardInfo {
                            pageservers: vec![PageserverShardConnectionInfo {
                                id: Some(id),
                                libpq_url: Some(url),
                                grpc_url: None,
                            }],
                        },
                    )]),
                    prefer_protocol: "libpq".to_owned(),
                }),
            ),
            None => (None, None),
        };
        ComputeSpec {
            format_version: 1.0,
            operation_uuid: self.compute_id,
            suspend_timeout_seconds: self.suspend_timeout_seconds,
            cluster: Cluster {
                cluster_id: self.project_id.clone(),
                name: self.branch_id.clone(),
                state: Some("restarted".to_owned()),
                roles: self
                    .roles
                    .into_iter()
                    .map(|r| Role {
                        name: r.name,
                        encrypted_password: r.encrypted_password.map(Secret::new),
                        options: None,
                    })
                    .collect(),
                databases: self
                    .databases
                    .into_iter()
                    .map(|d| Database {
                        name: d.name,
                        owner: d.owner,
                        options: None,
                    })
                    .collect(),
                settings: Some(settings),
            },
            delta_operations: None,
            tenant_id: self.tenant_id,
            timeline_id: self.timeline_id,
            pageserver_connstring,
            pageserver_connection_info,
            project_id: self.project_id,
            branch_id: self.branch_id,
            endpoint_id: self.endpoint_id,
            safekeeper_connstrings: self.safekeepers,
            mode: self.mode,
            storage_auth_token: self.storage_auth_token,
            endpoint_storage_addr: self.endpoint_storage.as_ref().map(|(a, _)| a.clone()),
            endpoint_storage_token: self.endpoint_storage.map(|(_, t)| t),
            autoprewarm: self.autoprewarm,
        }
    }
}
