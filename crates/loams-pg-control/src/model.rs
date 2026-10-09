//! The records of `pg-control` and their keys (design §46 §6.2, D705).
//!
//! | Record | Key |
//! |---|---|
//! | [`ProjectRec`] | `x/<ns>/<project_id>` |
//! | [`ProjectNameRec`] (the name index) | `x/<ns>/n/<name>` |
//! | [`BranchRec`] | `X/<project_id>/<branch_id>` |
//! | [`EndpointRec`] | `E/<project_id>/<endpoint_id>` |
//! | [`ComputeRec`] | `C/<compute_id>` |
//! | [`RoleRec`] | `R/<branch_id>/<role>` |
//! | [`DatabaseRec`] | `D/<branch_id>/<db>` |
//!
//! Keys are relative to the store's root: on TiKV, the metastore's keyspace
//! and root, beside `loams-meta-tikv`'s own keys, whose tags these never
//! share (`tags_are_disjoint_from_the_metastore`). A key is a one-letter
//! tag, `/`, then its parts joined by `/`. Every part but the last is an id
//! or a namespace and may not contain `/`; the last (a name) may. A project
//! listing's prefix is `x/<ns>/prj-`, so it never meets the name index
//! `x/<ns>/n/` (Task 4's ids all start with `prj-`).
//!
//! A stored value is a format byte ([`FORMAT`]), the record's version as a
//! postcard varint, then the record's postcard encoding (`store::encode`).
//! Postcard is not self-describing: a change to a record's fields is a new
//! format byte.
//!
//! Ids are the prefixed strings of §46 §3 (`prj-`, `br-`, `ep-`, `cmp-` and
//! a ULID); Task 4's newtypes serialize as the same strings, so their
//! encoding does not change.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::store::StoreError;

/// The format byte in front of every stored record.
pub const FORMAT: u8 = 1;

/// The tags of `pg-control`'s keys (§46 §6.2), and its lease scope's.
pub const TAGS: [u8; 6] = *b"xXECRD";

/// The longest part of a key, in bytes.
pub const MAX_PART_LEN: usize = 255;

/// A record of the store: its key, the prefix its listings take, and the
/// key's encoding.
pub trait Record: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    /// What names one record.
    type Key: Send + Sync;
    /// What names a listing of records.
    type Prefix: Send + Sync;
    /// The record's name in errors.
    const KIND: &'static str;
    /// This record's key.
    fn key(&self) -> Self::Key;
    /// The key's bytes, relative to the store's root.
    ///
    /// # Errors
    ///
    /// `InvalidArgument` when a part is empty, too long, or (but for the
    /// last part) contains `/`.
    fn encode_key(key: &Self::Key) -> Result<Vec<u8>, StoreError>;
    /// The listing prefix's bytes.
    ///
    /// # Errors
    ///
    /// As [`encode_key`](Self::encode_key).
    fn encode_prefix(prefix: &Self::Prefix) -> Result<Vec<u8>, StoreError>;
    /// The project this record belongs to, when it names one: a fenced
    /// write of it needs that project's lease, `e/pg/<project_id>` (R3.11).
    /// `None` (roles and databases, whose records name only a branch): any
    /// project's fence may write it.
    fn project(&self) -> Option<&str>;
}

/// `tag/part/…/part`; every part checked, the last allowed to hold `/`.
fn key(tag: u8, parts: &[&str]) -> Result<Vec<u8>, StoreError> {
    let last = parts.len().saturating_sub(1);
    let mut out = vec![tag, b'/'];
    for (i, part) in parts.iter().enumerate() {
        check_part(part, i == last)?;
        if i > 0 {
            out.push(b'/');
        }
        out.extend_from_slice(part.as_bytes());
    }
    Ok(out)
}

/// `tag/part/…/part/` followed by `tail` (a listing prefix).
fn prefix(tag: u8, parts: &[&str], tail: &str) -> Result<Vec<u8>, StoreError> {
    let mut out = vec![tag, b'/'];
    for part in parts {
        check_part(part, false)?;
        out.extend_from_slice(part.as_bytes());
        out.push(b'/');
    }
    out.extend_from_slice(tail.as_bytes());
    Ok(out)
}

fn check_part(part: &str, last: bool) -> Result<(), StoreError> {
    if part.is_empty() {
        return Err(StoreError::InvalidArgument("a key part is empty".into()));
    }
    if part.len() > MAX_PART_LEN {
        return Err(StoreError::InvalidArgument(format!(
            "a key part is longer than {MAX_PART_LEN} bytes"
        )));
    }
    if !last && part.contains('/') {
        return Err(StoreError::InvalidArgument(
            "an id or namespace in a key contains '/'".into(),
        ));
    }
    Ok(())
}

/// The lease scope of a project's reconciler: `e/pg/<project_id>` (§46
/// §6.3).
pub fn project_lease(project_id: &str) -> String {
    format!("{}{project_id}", crate::store::LEASE_SCOPE)
}

// ---- Shared value types ----

/// Compute units in steps of 0.25 CU (§46 §7.4): `Cu(4)` is 1 CU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Cu(pub u16);

/// A pool mode of PgDog (§46 §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PoolMode {
    Transaction,
    Session,
}

/// Which WAL service a project's branches write to (§46 §9: `loams-wal`
/// only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalService {
    /// A `loams-wal` pool, by name.
    LoamsWal { pool: String },
}

// ---- Project ----

/// A project's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProjectState {
    Creating,
    Ready,
    Deleting,
    Failed,
}

/// A project: one Neon tenant (§46 §3; §28 §5.3's database record).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRec {
    pub namespace: String,
    pub id: String,
    pub name: String,
    /// `SHA-256("loams/pg/tenant/" ‖ project_id)[0..16]` (Task 4).
    pub tenant_id: [u8; 16],
    pub pg_version: u32,
    pub wal: WalService,
    pub history_retention_s: u64,
    pub region: String,
    pub default_branch_id: Option<String>,
    pub settings: BTreeMap<String, String>,
    pub state: ProjectState,
    pub created_at_ms: u64,
}

/// `(namespace, project_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectKey {
    pub namespace: String,
    pub id: String,
}

/// The projects of a namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectPrefix {
    pub namespace: String,
}

impl Record for ProjectRec {
    type Key = ProjectKey;
    type Prefix = ProjectPrefix;
    const KIND: &'static str = "project";
    fn project(&self) -> Option<&str> {
        Some(&self.id)
    }
    fn key(&self) -> ProjectKey {
        ProjectKey {
            namespace: self.namespace.clone(),
            id: self.id.clone(),
        }
    }
    fn encode_key(k: &ProjectKey) -> Result<Vec<u8>, StoreError> {
        if !k.id.starts_with("prj-") {
            return Err(StoreError::InvalidArgument(
                "a project id starts with 'prj-'".into(),
            ));
        }
        key(b'x', &[&k.namespace, &k.id])
    }
    fn encode_prefix(p: &ProjectPrefix) -> Result<Vec<u8>, StoreError> {
        prefix(b'x', &[&p.namespace], "prj-")
    }
}

/// The name index of projects: a name is unique in its namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectNameRec {
    pub namespace: String,
    pub name: String,
    pub project_id: String,
}

/// `(namespace, name)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectNameKey {
    pub namespace: String,
    pub name: String,
}

impl Record for ProjectNameRec {
    type Key = ProjectNameKey;
    type Prefix = ProjectPrefix;
    const KIND: &'static str = "project name";
    fn project(&self) -> Option<&str> {
        Some(&self.project_id)
    }
    fn key(&self) -> ProjectNameKey {
        ProjectNameKey {
            namespace: self.namespace.clone(),
            name: self.name.clone(),
        }
    }
    fn encode_key(k: &ProjectNameKey) -> Result<Vec<u8>, StoreError> {
        key(b'x', &[&k.namespace, "n", &k.name])
    }
    fn encode_prefix(p: &ProjectPrefix) -> Result<Vec<u8>, StoreError> {
        prefix(b'x', &[&p.namespace, "n"], "")
    }
}

// ---- Branch ----

/// A branch's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BranchState {
    Creating,
    Ready,
    Deleting,
    Failed,
}

/// One shard's pageserver, from the storage controller's `notify-attach`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardAttachment {
    pub shard_number: u8,
    pub node_id: u64,
}

/// A branch: one timeline (§46 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRec {
    pub project_id: String,
    pub id: String,
    pub name: String,
    /// `SHA-256("loams/pg/timeline/" ‖ branch_id)[0..16]` (Task 4).
    pub timeline_id: [u8; 16],
    pub parent_id: Option<String>,
    /// The parent's LSN the branch starts at.
    pub ancestor_lsn: Option<u64>,
    /// When the branch expires, if it has a TTL.
    pub expires_at_ms: Option<u64>,
    pub protected: bool,
    pub stripe_size: Option<u32>,
    pub shards: Vec<ShardAttachment>,
    pub state: BranchState,
    pub created_at_ms: u64,
}

/// `(project_id, branch_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BranchKey {
    pub project_id: String,
    pub id: String,
}

/// The branches of a project.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BranchPrefix {
    pub project_id: String,
}

impl Record for BranchRec {
    type Key = BranchKey;
    type Prefix = BranchPrefix;
    const KIND: &'static str = "branch";
    fn project(&self) -> Option<&str> {
        Some(&self.project_id)
    }
    fn key(&self) -> BranchKey {
        BranchKey {
            project_id: self.project_id.clone(),
            id: self.id.clone(),
        }
    }
    fn encode_key(k: &BranchKey) -> Result<Vec<u8>, StoreError> {
        key(b'X', &[&k.project_id, &k.id])
    }
    fn encode_prefix(p: &BranchPrefix) -> Result<Vec<u8>, StoreError> {
        prefix(b'X', &[&p.project_id], "")
    }
}

// ---- Endpoint ----

/// `read_write` (one per branch) or `read_only`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EndpointType {
    ReadWrite,
    ReadOnly,
}

/// What the endpoint should be doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DesiredState {
    Running,
    Suspended,
}

/// The compute state machine's state (§46 §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EndpointState {
    Idle,
    Starting,
    Running,
    Suspending,
    Suspended,
    Failed,
    Deleting,
}

/// An endpoint: a stable address and its compute settings (§46 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointRec {
    pub project_id: String,
    pub id: String,
    pub branch_id: String,
    pub kind: EndpointType,
    pub min_cu: Cu,
    pub max_cu: Cu,
    /// 0: never suspend.
    pub suspend_timeout_s: u32,
    pub pool_mode: PoolMode,
    pub pg_settings: BTreeMap<String, String>,
    pub desired: DesiredState,
    pub state: EndpointState,
    pub compute_id: Option<String>,
    /// `host:port` of the Service or host port.
    pub backend_addr: Option<String>,
    /// Why the endpoint is `Failed`.
    pub failure: Option<String>,
}

/// `(project_id, endpoint_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointKey {
    pub project_id: String,
    pub id: String,
}

/// The endpoints of a project.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointPrefix {
    pub project_id: String,
}

impl Record for EndpointRec {
    type Key = EndpointKey;
    type Prefix = EndpointPrefix;
    const KIND: &'static str = "endpoint";
    fn project(&self) -> Option<&str> {
        Some(&self.project_id)
    }
    fn key(&self) -> EndpointKey {
        EndpointKey {
            project_id: self.project_id.clone(),
            id: self.id.clone(),
        }
    }
    fn encode_key(k: &EndpointKey) -> Result<Vec<u8>, StoreError> {
        key(b'E', &[&k.project_id, &k.id])
    }
    fn encode_prefix(p: &EndpointPrefix) -> Result<Vec<u8>, StoreError> {
        prefix(b'E', &[&p.project_id], "")
    }
}

// ---- Compute ----

/// A compute's status as `pg-control` last saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ComputeStatus {
    /// Started, waiting for its spec.
    Pending,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// One compute process (§46 §3; §28 §5.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputeRec {
    pub id: String,
    pub project_id: String,
    pub endpoint_id: String,
    pub spec_version: u64,
    pub status: ComputeStatus,
    /// The id of the key its JWT was signed with (never the key).
    pub jwt_key_id: Option<String>,
    pub cu: Cu,
    pub created_at_ms: u64,
    pub last_active_ms: Option<u64>,
}

/// A compute id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ComputeKey {
    pub id: String,
}

/// Every compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AllComputes;

impl Record for ComputeRec {
    type Key = ComputeKey;
    type Prefix = AllComputes;
    const KIND: &'static str = "compute";
    fn project(&self) -> Option<&str> {
        Some(&self.project_id)
    }
    fn key(&self) -> ComputeKey {
        ComputeKey {
            id: self.id.clone(),
        }
    }
    fn encode_key(k: &ComputeKey) -> Result<Vec<u8>, StoreError> {
        key(b'C', &[&k.id])
    }
    fn encode_prefix(_: &AllComputes) -> Result<Vec<u8>, StoreError> {
        prefix(b'C', &[], "")
    }
}

// ---- Role and database ----

/// A Postgres role of a branch. The secret lives in the credential store;
/// this holds only its reference (§46 §8.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleRec {
    pub branch_id: String,
    pub name: String,
    pub secret_ref: String,
    pub login: bool,
    pub pool_mode: Option<PoolMode>,
}

/// `(branch_id, role)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoleKey {
    pub branch_id: String,
    pub name: String,
}

/// The roles, or the databases, of a branch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BranchScope {
    pub branch_id: String,
}

impl Record for RoleRec {
    type Key = RoleKey;
    type Prefix = BranchScope;
    const KIND: &'static str = "role";
    fn project(&self) -> Option<&str> {
        None
    }
    fn key(&self) -> RoleKey {
        RoleKey {
            branch_id: self.branch_id.clone(),
            name: self.name.clone(),
        }
    }
    fn encode_key(k: &RoleKey) -> Result<Vec<u8>, StoreError> {
        key(b'R', &[&k.branch_id, &k.name])
    }
    fn encode_prefix(p: &BranchScope) -> Result<Vec<u8>, StoreError> {
        prefix(b'R', &[&p.branch_id], "")
    }
}

/// A database of a branch; its owner is a role of the same branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseRec {
    pub branch_id: String,
    pub name: String,
    pub owner: String,
}

/// `(branch_id, database)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DatabaseKey {
    pub branch_id: String,
    pub name: String,
}

impl Record for DatabaseRec {
    type Key = DatabaseKey;
    type Prefix = BranchScope;
    const KIND: &'static str = "database";
    fn project(&self) -> Option<&str> {
        None
    }
    fn key(&self) -> DatabaseKey {
        DatabaseKey {
            branch_id: self.branch_id.clone(),
            name: self.name.clone(),
        }
    }
    fn encode_key(k: &DatabaseKey) -> Result<Vec<u8>, StoreError> {
        key(b'D', &[&k.branch_id, &k.name])
    }
    fn encode_prefix(p: &BranchScope) -> Result<Vec<u8>, StoreError> {
        prefix(b'D', &[&p.branch_id], "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bkey(project_id: &str, id: &str) -> Vec<u8> {
        BranchRec::encode_key(&BranchKey {
            project_id: project_id.into(),
            id: id.into(),
        })
        .expect("a key")
    }

    #[test]
    fn keys_follow_the_design_table() {
        let p = ProjectRec::encode_key(&ProjectKey {
            namespace: "acme".into(),
            id: "prj-1".into(),
        })
        .expect("a key");
        assert_eq!(p, b"x/acme/prj-1");
        let n = ProjectNameRec::encode_key(&ProjectNameKey {
            namespace: "acme".into(),
            name: "shop".into(),
        })
        .expect("a key");
        assert_eq!(n, b"x/acme/n/shop");
        assert_eq!(bkey("prj-1", "br-2"), b"X/prj-1/br-2");
        let r = RoleRec::encode_key(&RoleKey {
            branch_id: "br-2".into(),
            name: "a/b".into(),
        })
        .expect("a role name may hold '/'");
        assert_eq!(r, b"R/br-2/a/b");
        assert_eq!(
            ComputeRec::encode_prefix(&AllComputes).expect("a prefix"),
            b"C/"
        );
        assert_eq!(project_lease("prj-1"), "e/pg/prj-1");
    }

    #[test]
    fn project_listing_never_meets_the_name_index() {
        let listing = ProjectRec::encode_prefix(&ProjectPrefix {
            namespace: "acme".into(),
        })
        .expect("a prefix");
        let index = ProjectNameRec::encode_prefix(&ProjectPrefix {
            namespace: "acme".into(),
        })
        .expect("a prefix");
        assert!(!index.starts_with(&listing) && !listing.starts_with(&index));
        assert!(
            ProjectRec::encode_key(&ProjectKey {
                namespace: "acme".into(),
                id: "n".into(),
            })
            .is_err()
        );
    }

    #[test]
    fn a_prefix_holds_only_its_parents_keys() {
        let of_one = BranchRec::encode_prefix(&BranchPrefix {
            project_id: "prj-1".into(),
        })
        .expect("a prefix");
        assert!(bkey("prj-1", "br-2").starts_with(&of_one));
        assert!(!bkey("prj-10", "br-2").starts_with(&of_one));
    }

    #[test]
    fn bad_parts_are_refused() {
        for (p, b) in [("", "br-1"), ("prj/1", "br-1"), ("prj-1", "")] {
            let e = BranchRec::encode_key(&BranchKey {
                project_id: p.into(),
                id: b.into(),
            });
            assert!(
                matches!(e, Err(StoreError::InvalidArgument(_))),
                "{p:?} {b:?}"
            );
        }
        let long = "a".repeat(MAX_PART_LEN + 1);
        assert!(bkey_checked("prj-1", &long).is_err());
    }

    fn bkey_checked(project_id: &str, id: &str) -> Result<Vec<u8>, StoreError> {
        BranchRec::encode_key(&BranchKey {
            project_id: project_id.into(),
            id: id.into(),
        })
    }

    /// `loams-meta-tikv`'s tags (`crates/loams-meta-tikv/src/keys.rs`) and
    /// its lease scopes (`e/m/`, `e/cluster/`), and the runner's commit
    /// tokens (`t/`), as of Task 3. The pg keys share the metastore's root,
    /// so neither may ever read the other's records.
    #[test]
    fn tags_are_disjoint_from_the_metastore() {
        let metastore = b"achHikKlLnNopqrsSwWet";
        for tag in TAGS {
            assert!(!metastore.contains(&tag), "tag {}", tag as char);
        }
        let scope = crate::store::LEASE_SCOPE.as_bytes();
        for theirs in [b"e/m/".as_slice(), b"e/cluster/"] {
            assert!(!scope.starts_with(theirs) && !theirs.starts_with(scope));
        }
    }
}
