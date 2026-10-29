//! PgDog, as `pg-control` talks to it (design §46 §8): through its admin
//! database, one Deployment per Loams namespace. The seam is [`PgDogAdmin`]
//! (the shared contract's RT1 adapter, extended): [`admin`] implements it
//! over PgDog's admin database, and [`NoPgDog`] is a deployment with no
//! PgDog in front of its endpoints. [`render`] turns a namespace's records
//! into PgDog's two files (Task 20) and [`push`] delivers them (Task 21).
//! The idle monitor (Task 12) reads the clients of an endpoint's pools
//! through the seam; the waker's polling is Task 26's.

pub mod admin;
pub mod push;
pub mod render;

use async_trait::async_trait;

use crate::model::{BranchRec, EndpointRec, EndpointType, ProjectRec};
use crate::names::{Routed, parse_routed};

/// One PgDog replica (a Pod on Kubernetes, a container on compose).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId(pub String);

/// One row of `SHOW POOLS`: a (routed database, user) pool and its
/// clients and server connections. Task 18 checks the columns against the
/// pinned PgDog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PoolRow {
    /// The routed name (§46 §8.1), PgDog's `[[databases]]` `name`.
    pub database: String,
    pub user: String,
    /// Clients waiting for a server connection.
    pub cl_waiting: u64,
    /// Server connections in use: a client's transaction (or, in session
    /// mode, its session), so a query running counts for as long as it
    /// runs. The pinned PgDog's `SHOW POOLS` has no `cl_active` (R18.7).
    pub sv_active: u64,
}

/// Why PgDog did not answer. The text is for logs only (it can name hosts).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("pgdog: {0}")]
pub struct PgDogError(pub String);

/// PgDog's admin database (the shared contract's `PgDogAdmin`).
///
/// `instances` is an addition to the contract (R12.3): the idle monitor
/// and the waker need to know which replicas serve a namespace before they
/// can read their pools.
#[async_trait]
pub trait PgDogAdmin: Send + Sync + 'static {
    /// The replicas serving `namespace`; none when no PgDog fronts it.
    async fn instances(&self, namespace: &str) -> Result<Vec<InstanceId>, PgDogError>;
    /// `RELOAD`.
    async fn reload(&self, instance: &InstanceId) -> Result<(), PgDogError>;
    /// `SHOW POOLS`.
    async fn show_pools(&self, instance: &InstanceId) -> Result<Vec<PoolRow>, PgDogError>;
    /// `SHOW DATABASES`: the routed names the replica serves.
    async fn show_databases(&self, instance: &InstanceId) -> Result<Vec<String>, PgDogError>;
}

/// No PgDog in front of any endpoint: every namespace has no replicas, so
/// no endpoint has PgDog clients. What `pg-control` runs with until PG2c
/// renders and deploys PgDog (Tasks 20–22).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPgDog;

#[async_trait]
impl PgDogAdmin for NoPgDog {
    async fn instances(&self, _: &str) -> Result<Vec<InstanceId>, PgDogError> {
        Ok(Vec::new())
    }
    async fn reload(&self, instance: &InstanceId) -> Result<(), PgDogError> {
        Err(PgDogError(format!("no PgDog replica {}", instance.0)))
    }
    async fn show_pools(&self, instance: &InstanceId) -> Result<Vec<PoolRow>, PgDogError> {
        Err(PgDogError(format!("no PgDog replica {}", instance.0)))
    }
    async fn show_databases(&self, instance: &InstanceId) -> Result<Vec<String>, PgDogError> {
        Err(PgDogError(format!("no PgDog replica {}", instance.0)))
    }
}

/// Whether the pool of routed name `database` sends its clients to the
/// endpoint `ep` of branch `branch` in `project` (§46 §8.1's table):
/// a read-write endpoint serves `<project>` (when its branch is the
/// default), `<project>__<branch>` and `<project>__<branch>.<database>`;
/// read-only endpoints serve `<project>__<branch>__ro`, load-balanced, so
/// each counts the clients of all of them. A name that does not parse
/// serves no endpoint.
pub fn pool_serves(
    database: &str,
    project: &ProjectRec,
    branch: &BranchRec,
    ep: &EndpointRec,
) -> bool {
    let Ok(routed) = parse_routed(database) else {
        return false;
    };
    if routed.project() != project.name {
        return false;
    }
    match (ep.kind, &routed) {
        (EndpointType::ReadOnly, Routed::ReadOnly { branch: b, .. }) => *b == branch.name,
        (EndpointType::ReadWrite, Routed::Project { .. }) => {
            project.default_branch_id.as_deref() == Some(branch.id.as_str())
        }
        (
            EndpointType::ReadWrite,
            Routed::Branch { branch: b, .. } | Routed::Database { branch: b, .. },
        ) => *b == branch.name,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::model::{BranchState, ProjectState, WalService};

    fn project() -> ProjectRec {
        ProjectRec {
            namespace: "acme".into(),
            id: "prj-a".into(),
            name: "shop".into(),
            tenant_id: [0; 16],
            pg_version: 17,
            wal: WalService::LoamsWal {
                pool: "default".into(),
            },
            history_retention_s: 0,
            region: "local".into(),
            default_branch_id: Some("br-main".into()),
            settings: BTreeMap::new(),
            state: ProjectState::Ready,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn branch(id: &str, name: &str) -> BranchRec {
        BranchRec {
            project_id: "prj-a".into(),
            id: id.into(),
            name: name.into(),
            timeline_id: [0; 16],
            parent_id: None,
            ancestor_lsn: None,
            expires_at_ms: None,
            protected: false,
            stripe_size: None,
            shards: Vec::new(),
            state: BranchState::Ready,
            created_at_ms: 0,
            parent_time_ms: None,
            updated_at_ms: 0,
            pg_version: 17,
        }
    }

    #[test]
    fn pools_map_to_their_endpoints_by_the_routed_name() {
        let p = project();
        let main = branch("br-main", "main");
        let dev = branch("br-dev", "dev");
        let rw = EndpointRec::new("prj-a", "ep-rw", "br-main", EndpointType::ReadWrite);
        let ro = EndpointRec::new("prj-a", "ep-ro", "br-main", EndpointType::ReadOnly);
        let dev_rw = EndpointRec::new("prj-a", "ep-dev", "br-dev", EndpointType::ReadWrite);

        for name in ["shop", "shop__main", "shop__main.app"] {
            assert!(pool_serves(name, &p, &main, &rw), "{name}");
            assert!(!pool_serves(name, &p, &main, &ro), "{name}");
            assert!(!pool_serves(name, &p, &dev, &dev_rw), "{name}");
        }
        assert!(pool_serves("shop__main__ro", &p, &main, &ro));
        assert!(!pool_serves("shop__main__ro", &p, &main, &rw));
        assert!(pool_serves("shop__dev.app", &p, &dev, &dev_rw));
        // Another project, or a name that does not parse, serves none.
        assert!(!pool_serves("other__main", &p, &main, &rw));
        assert!(!pool_serves("Shop", &p, &main, &rw));
        assert!(!pool_serves("shop__main__x", &p, &main, &rw));
    }
}
