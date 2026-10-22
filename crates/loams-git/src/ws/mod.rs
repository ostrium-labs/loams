//! Server-side workspaces (§36 §19.4, D826, D827, D836; plan GT1 Tasks
//! 23–27).
//!
//! A workspace is a create-only chain of complete snapshots,
//! `ws/<ws_id>/snap/<n:020>.lws` (Task 22's format). The snapshot PUT is the
//! commit point and the fence: at most one snapshot is ever stored under an
//! `n`, so two mounts writing one workspace cannot both save `n` (Ruling 31:
//! no lease service). [`WorkspaceStore`] is the contract;
//! [`BucketWorkspaceStore`] keeps it on the bucket and [`MemWorkspaceStore`]
//! in memory, for tests.
//!
//! A save builds the trees along its changed paths ([`TreeBuilder`]) and
//! uploads the new objects as one workspace pack ([`put_workspace_pack`],
//! Task 24). `WorkspaceService` (Task 25) checks a save's objects
//! ([`check_save_objects`]), replays salvage ([`replay_salvage`]) and finds
//! workspaces for object reads ([`StoreWsLookup`]). Task 27 commits a
//! workspace's index on the server ([`workspace_commit`]) and pushes its
//! commits without re-uploading objects ([`push_workspace`]).

mod commit;
mod edits;
mod lookup;
pub mod merge3;
mod pack;
mod pack_write;
mod push;
mod salvage;
mod save;
mod store;
mod tree_build;

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use loams_store::StoreError;
use ulid::Ulid;

use crate::format::FormatError;
use crate::format::pb;
use crate::format::snapshot::snapshot_commit;
use crate::ids::{MountId, ObjectId, Principal, RefName, RepoId, Seq, SnapN, WsId, WsName};

pub use commit::{Ident, SERVER_COMMITTER, same_workspace_commit, workspace_commit};
pub use edits::{
    DIFF_CONTEXT, EditError, Edited, EnvHooks, EnvRefusal, FileEdit, FileRead, NoEnv, Rebased,
    Rendered, apply_edits, changed_paths, file_patch, is_env_path, read_file, rebase_trees,
};
pub use lookup::{StoreWsLookup, view_of};
pub use merge3::{Merge3, MergeLabels, merge3};
pub use pack::{
    NoScan, PackScan, ScanRefusal, VerifiedPack, WsPackBlobs, WsPackError, WsPackUploads,
    check_pack_len, fork_sources, put_workspace_pack, verify_workspace_pack,
};
pub use pack_write::{PackEntry, write_ws_pack};
pub use push::{
    ANCESTRY_CLOCK_SKEW_SECS, Lease, PushError, PushLog, PushRequest, PushSpec, Pushed, RefStatus,
    is_fast_forward, push_closure, push_key, push_workspace,
};
pub use salvage::{ReplayError, Replayed, replay_salvage};
pub use save::{SaveCheckError, check_save_objects, commit_tree, empty_tree};
pub use store::{BucketWorkspaceStore, MemWorkspaceStore};
pub use tree_build::{Edit, FileMode, TreeBuildError, TreeBuilder};

/// Where a new workspace starts (§19.4.4): its snapshot 0 has
/// `base = head = commit` and `index_tree = worktree_tree = tree`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WsBase {
    /// What `pull` rebases onto, such as `refs/heads/main`.
    pub track: RefName,
    /// The commit started from (`track`'s tip at `seq` by default); `None`
    /// for a repository with no commit yet (an unborn workspace).
    pub commit: Option<ObjectId>,
    /// The commit's tree; the empty tree for an unborn workspace.
    pub tree: ObjectId,
    /// The repository seq `commit` was read at.
    pub seq: Seq,
    /// What `HEAD` names; `None` when detached.
    pub head_ref: Option<RefName>,
}

/// A workspace to create or fork (§19.4.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateWs {
    /// The repository.
    pub repo: RepoId,
    /// The new workspace's id, chosen by the caller ([`WsId::generate`]).
    /// Creating is idempotent on it: a retry with the same spec answers the
    /// snapshot 0 the first attempt wrote.
    pub ws_id: WsId,
    /// Its owner, the principal's stable id.
    pub owner: Principal,
    /// Its name, unique per owner; `None` for an unnamed (agent) workspace,
    /// which has no name record.
    pub name: Option<WsName>,
    /// Who asked (the snapshot's `principal`).
    pub principal: Principal,
    /// The mount that will write it, when one is known (the snapshot's
    /// `writer`); `None` writes `""`.
    pub writer: Option<MountId>,
    /// Where it starts: required by [`WorkspaceStore::create`]; a fork keeps
    /// its source's state, so [`WorkspaceStore::fork`] requires `None`.
    pub base: Option<WsBase>,
}

/// The outcome of [`WorkspaceStore::append`] (plan "Shared contracts").
#[derive(Clone, Debug, PartialEq)]
pub enum SnapAppended {
    /// The snapshot owns `n`, now or from an earlier attempt by the same
    /// writer with the same bytes.
    Committed {
        /// The snapshot's number.
        n: SnapN,
    },
    /// Another snapshot owns `n`: the writer is fenced (§19.4.3).
    Fenced {
        /// The snapshot stored at `n`.
        existing: Arc<pb::WorkspaceSnapshot>,
    },
}

/// One workspace in a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WsSummary {
    /// Its id.
    pub ws_id: WsId,
    /// Its name; set when listing an owner's named workspaces.
    pub name: Option<WsName>,
    /// When its name was taken; set with `name`.
    pub created_unix_ms: Option<i64>,
}

impl WsSummary {
    /// The `after` that continues a listing past this entry: the name in an
    /// owner's listing, the id in a listing of every workspace.
    pub fn cursor(&self) -> &str {
        match &self.name {
            Some(name) => name.as_str(),
            None => self.ws_id.as_str(),
        }
    }
}

/// The label of the final snapshot `DeleteWorkspace` writes (§19.4.5): a
/// workspace whose latest snapshot carries it is deleted, and only its
/// purge (after `GC_GRACE`) remains.
pub const DELETED_LABEL: &str = "deleted";

/// One salvage record, `salvage/<mount>/<id>.lws` (§19.4.3).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SalvageRef {
    /// The fenced mount whose unsaved state it holds.
    pub mount: MountId,
    /// Its id, a ULID generated when it was written.
    pub id: Ulid,
}

/// Errors of a [`WorkspaceStore`].
#[derive(Debug, thiserror::Error)]
pub enum WsError {
    /// The workspace, or the snapshot asked for, does not exist.
    #[error("workspace not found")]
    NotFound,
    /// The owner already has a workspace of that name under another id
    /// (`workspace_name_taken`, `already_exists`).
    #[error("the owner already has a workspace named {name}")]
    WorkspaceNameTaken {
        /// The name.
        name: WsName,
        /// The workspace that holds it.
        holder: WsId,
    },
    /// The workspace id already names a different workspace (another owner,
    /// name or repository): ids are generated, so a caller reused one.
    #[error("workspace {ws_id} exists with a different owner, name or repository")]
    IdConflict {
        /// The id.
        ws_id: WsId,
    },
    /// A snapshot or spec that cannot be stored.
    #[error("invalid workspace request: {reason}")]
    Invalid {
        /// What is wrong.
        reason: String,
    },
    /// The encoded snapshot body is over `MAX_SNAPSHOT_BYTES` (1 MiB).
    #[error("snapshot body over 1 MiB")]
    TooLarge,
    /// A stored object cannot be read, or the store answered impossibly.
    #[error("corrupt workspace object {path}: {reason}")]
    Corrupt {
        /// The object's path.
        path: String,
        /// What is wrong.
        reason: String,
    },
    /// The object store failed (after retries, when transient).
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl WsError {
    fn invalid(reason: impl ToString) -> Self {
        Self::Invalid {
            reason: reason.to_string(),
        }
    }

    fn corrupt(path: &str, reason: impl ToString) -> Self {
        Self::Corrupt {
            path: path.to_string(),
            reason: reason.to_string(),
        }
    }

    fn from_format(err: FormatError) -> Self {
        match err {
            FormatError::TooLarge { .. } => Self::TooLarge,
            other => Self::invalid(other),
        }
    }
}

/// The workspaces of the repositories in one namespace (§36 §19.4; plan
/// "Shared contracts").
#[async_trait]
pub trait WorkspaceStore: Send + Sync + fmt::Debug {
    /// Writes the name record (if named, put_if_absent) and snapshot 0. Idempotent on `spec.ws_id`.
    ///
    /// Errors: `WorkspaceNameTaken` when another workspace holds the name,
    /// `IdConflict` when `ws_id` names a workspace of another spec,
    /// `Invalid` without `spec.base`.
    async fn create(&self, spec: CreateWs) -> Result<Arc<pb::WorkspaceSnapshot>, WsError>;
    /// Hint, then probe forward until 404. Linearizable at return.
    ///
    /// Without a usable hint the snapshot names are listed first. `NotFound`
    /// when the workspace has no snapshot.
    async fn latest(&self, repo: &RepoId, ws: &WsId)
    -> Result<Arc<pb::WorkspaceSnapshot>, WsError>;
    /// Snapshot `n`, or `None` when it does not exist (or was collected).
    async fn get(
        &self,
        repo: &RepoId,
        ws: &WsId,
        n: SnapN,
    ) -> Result<Option<Arc<pb::WorkspaceSnapshot>>, WsError>;
    /// Create-only PUT of `snap.n`; a 412 reads back: same writer and same bytes = Committed.
    ///
    /// Precondition (caller's): snapshot `n - 1` exists. Snapshot 0 is
    /// written only by `create` and `fork`, so `n = 0` is `Invalid`.
    async fn append(&self, snap: &pb::WorkspaceSnapshot) -> Result<SnapAppended, WsError>;
    /// Stores a fenced writer's unsaved state under
    /// `salvage/<mount>/<ulid>.lws` (§19.4.3) and answers the ulid.
    async fn salvage(&self, snap: &pb::WorkspaceSnapshot, mount: &MountId)
    -> Result<Ulid, WsError>;
    /// A page of at most `limit` workspaces of `repo`, in path order after
    /// `after` (a [`WsSummary::cursor`]): `owner`'s named workspaces, or with
    /// `None` every workspace (for administrators and GC).
    async fn list(
        &self,
        repo: &RepoId,
        owner: Option<&Principal>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<WsSummary>, WsError>;
    /// Writes `spec`'s snapshot 0 as a copy of snapshot `n` of `from`, with
    /// `fork` set; the copy names the source's packs, which stay under the
    /// source's prefix (§19.4.4). One PUT for an unnamed fork; idempotent on
    /// `spec.ws_id` like `create`.
    async fn fork(
        &self,
        from: (&RepoId, &WsId, SnapN),
        spec: CreateWs,
    ) -> Result<Arc<pb::WorkspaceSnapshot>, WsError>;
    /// Writes the `head` hint `{n, written_unix_ms}`. Best effort and
    /// coalesced by the caller (≤ 1 write/s); `n` must be committed, so the
    /// hint is never ahead of the true tail.
    ///
    /// As built (ruling T23.1): an addition to the plan's trait, as
    /// `WalStore` has `publish_hint`.
    async fn publish_hint(&self, repo: &RepoId, ws: &WsId, n: SnapN) -> Result<(), WsError>;
    /// A page of at most `limit` of `ws`'s snapshot numbers, ascending,
    /// after `after` (`ListSnapshots`, Task 25). Collected snapshots are not
    /// listed.
    async fn list_snapshots(
        &self,
        repo: &RepoId,
        ws: &WsId,
        after: Option<SnapN>,
        limit: usize,
    ) -> Result<Vec<SnapN>, WsError>;
    /// A page of at most `limit` of `ws`'s salvage records, by mount and
    /// then id, after `after` (`ListSalvage`, Task 25).
    async fn list_salvage(
        &self,
        repo: &RepoId,
        ws: &WsId,
        after: Option<&SalvageRef>,
        limit: usize,
    ) -> Result<Vec<SalvageRef>, WsError>;
    /// The unsaved state salvage record `salvage` holds, or `None`.
    async fn get_salvage(
        &self,
        repo: &RepoId,
        ws: &WsId,
        salvage: &SalvageRef,
    ) -> Result<Option<Arc<pb::WorkspaceSnapshot>>, WsError>;
}

/// Snapshot 0 of a new workspace (§19.4.4): it writes no data and copies
/// nothing.
fn snapshot_zero(spec: &CreateWs, now_unix_ms: i64) -> Result<pb::WorkspaceSnapshot, WsError> {
    let base = spec
        .base
        .as_ref()
        .ok_or_else(|| WsError::invalid("creating a workspace needs a base"))?;
    let commit = base
        .commit
        .map(|oid| oid.as_bytes().to_vec())
        .unwrap_or_default();
    let tree = base.tree.as_bytes().to_vec();
    let mut snap = identity(spec, now_unix_ms);
    snap.track = base.track.to_string();
    snap.base = commit.clone();
    snap.base_seq = base.seq.0;
    snap.head = commit;
    snap.head_ref = base
        .head_ref
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    snap.index_tree = tree.clone();
    snap.worktree_tree = tree;
    seal_snapshot(snap)
}

/// Snapshot 0 of a fork of `source` (snapshot `from.2` of `from.1`): the
/// source's state and packs under the new identity, with `fork` set.
fn fork_zero(
    source: &pb::WorkspaceSnapshot,
    from: (&RepoId, &WsId, SnapN),
    spec: &CreateWs,
    now_unix_ms: i64,
) -> Result<pb::WorkspaceSnapshot, WsError> {
    if spec.base.is_some() {
        return Err(WsError::invalid(
            "a fork keeps its source's state: it takes no base",
        ));
    }
    if from.0 != &spec.repo {
        return Err(WsError::invalid(
            "a workspace is forked within its repository",
        ));
    }
    let id = identity(spec, now_unix_ms);
    let snap = pb::WorkspaceSnapshot {
        ws_id: id.ws_id,
        n: 0,
        owner: id.owner,
        name: id.name,
        writer: id.writer,
        principal: id.principal,
        created_unix_ms: id.created_unix_ms,
        label: String::new(),
        takeover: None,
        fork: Some(pb::ForkOrigin {
            repo_id: from.0.to_string(),
            ws_id: from.1.to_string(),
            n: from.2.0,
        }),
        ..source.clone()
    };
    seal_snapshot(snap)
}

/// The identity fields of a new workspace's snapshot 0.
fn identity(spec: &CreateWs, now_unix_ms: i64) -> pb::WorkspaceSnapshot {
    pb::WorkspaceSnapshot {
        ws_id: spec.ws_id.to_string(),
        n: 0,
        object_format: pb::ObjectFormat::Sha1 as i32,
        repo_id: spec.repo.to_string(),
        owner: spec.owner.to_string(),
        name: spec
            .name
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        writer: spec
            .writer
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        principal: spec.principal.to_string(),
        created_unix_ms: now_unix_ms,
        ..Default::default()
    }
}

/// Sets the snapshot's `snapshot_commit` (Ruling 38) from its fields.
pub fn seal_snapshot(mut snap: pb::WorkspaceSnapshot) -> Result<pb::WorkspaceSnapshot, WsError> {
    let (commit, _) = snapshot_commit(&snap).map_err(WsError::from_format)?;
    snap.snapshot_commit = commit.as_bytes().to_vec();
    Ok(snap)
}

/// Whether a stored snapshot 0 is the one `spec` would write (a replay):
/// the same repository, owner, name and fork origin. The rest depends on
/// the clock or the source read at the time.
fn is_replay(existing: &pb::WorkspaceSnapshot, offered: &pb::WorkspaceSnapshot) -> bool {
    existing.ws_id == offered.ws_id
        && existing.repo_id == offered.repo_id
        && existing.owner == offered.owner
        && existing.name == offered.name
        && existing.fork == offered.fork
}

/// The repository, workspace and number of a snapshot to append.
fn appended_ids(snap: &pb::WorkspaceSnapshot) -> Result<(RepoId, WsId, SnapN), WsError> {
    let repo = RepoId::parse(&snap.repo_id).map_err(WsError::invalid)?;
    let ws = WsId::parse(&snap.ws_id).map_err(WsError::invalid)?;
    if snap.n == 0 {
        return Err(WsError::invalid(
            "snapshot 0 is written by create or fork, never appended",
        ));
    }
    Ok((repo, ws, SnapN(snap.n)))
}
