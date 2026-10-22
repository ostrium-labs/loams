//! Pushing a workspace (§36 §19.7.5, D831; plan GT1 Task 27): the objects
//! arrived with the saves, so a push moves no data from the client.
//!
//! [`push_workspace`] checks each update against the repository's latest
//! refs (fast-forward, an existing tag, a lease), walks the closure of the
//! new tips the repository lacks through the workspace's packs, copies
//! those objects into one repository pack (stored entries are copied, deltas
//! resolved: [`super::pack_write`]'s copy mode), verifies and scans it as an
//! upload is ([`put_workspace_pack`]), stores it, and commits one
//! [`RefTxn`] through the repository's sequencer. When another push wins a
//! ref first, the updates are checked again against the new refs and the
//! transaction is retried (`MAX_FENCE_RETRIES`).

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use gix_object::Kind;
use rand::Rng as _;
use sha2::{Digest, Sha256};

use super::pack::{PackScan, WsPackError, check_pack_len, put_workspace_pack};
use super::pack_write::{PackEntry, write_pack};
use super::save::empty_tree;
use crate::blob::BlobStore;
use crate::format::pb;
use crate::ids::{IdemKey, ObjectId, Principal, RefName, RepoId, RepoPath, Seq};
use crate::limits::MAX_FENCE_RETRIES;
use crate::odb::{ObjectDb, OdbError, RangeOdb};
use crate::reflog::{
    Expect, ReadAt, Receipt, RefError, RefLog, RefSnapshot, RefTxn, RefUpdate, RejectReason,
};
use crate::state::pattern_matches;

/// Commits answered `Unavailable` (the outcome was lost) are repeated with
/// the same key this many times; a repeat of a landed commit replays it.
const COMMIT_ATTEMPTS: u32 = 5;

/// How far a commit's committer time may run behind its descendants' before
/// the fast-forward walk stops descending past it (ruling T27.4): commits
/// older than `old`'s time by more than this are not searched for `old`.
/// Clock skew beyond it can only refuse a fast-forward, never accept one.
pub const ANCESTRY_CLOCK_SKEW_SECS: i64 = 86_400;

/// What a push's update must find the remote ref at, beyond the usual
/// fast-forward rule (`--force-with-lease=<ref>:<expect>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lease {
    /// The ref must not exist.
    Absent,
    /// The ref must point here.
    Oid(ObjectId),
}

/// One ref a push updates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushSpec {
    /// The repository's ref.
    pub dst: RefName,
    /// What it will point at; `None` deletes it.
    pub new: Option<ObjectId>,
    /// Update even when it is not a fast-forward (`+`).
    pub force: bool,
    /// Update only when the ref is where the lease says; a leased update
    /// need not be a fast-forward.
    pub lease: Option<Lease>,
}

/// A push of a workspace's objects (`PushWorkspace`).
#[derive(Clone, Debug)]
pub struct PushRequest {
    /// The updates, each ref once.
    pub specs: Vec<PushSpec>,
    /// Every update or none (`git push --atomic`). Otherwise refs a check
    /// refuses are reported and the rest are committed.
    pub atomic: bool,
    /// Who pushed.
    pub principal: Principal,
    /// git push options.
    pub push_options: Vec<String>,
    /// The caller's idempotency key with the digest of the request it is
    /// bound to; `None` derives a key per transaction (§36 §4.5).
    pub key: Option<(IdemKey, [u8; 32])>,
}

/// What became of one ref.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefStatus {
    /// Moved (or created, or deleted).
    Updated {
        /// Where it pointed.
        old: Option<ObjectId>,
        /// Where it points; `None`: deleted.
        new: Option<ObjectId>,
    },
    /// Already where the push would put it.
    UpToDate,
    /// Not a fast-forward, and not forced.
    NonFastForward,
    /// The ref points at a commit the workspace does not have: fetch (or
    /// rebase) first.
    FetchFirst,
    /// The lease does not hold, or the ref kept changing under every retry.
    Stale {
        /// Where the ref points.
        current: Option<ObjectId>,
    },
    /// A tag that exists is never moved without force; or a create of a ref
    /// that exists.
    AlreadyExists,
    /// A protection rule refuses the delete or the forced update.
    Protected,
    /// The writer's scope does not cover a path the update changes.
    OutOfScope {
        /// The first such path.
        path: RepoPath,
    },
    /// A delete of a ref that does not exist.
    NotFound,
    /// The ref name is not valid here.
    Invalid,
    /// Another ref of an atomic push was refused.
    AtomicFailed,
}

impl RefStatus {
    /// Whether the ref is where the push asked (moved now, or already).
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Updated { .. } | Self::UpToDate)
    }
}

/// A push's outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct Pushed {
    /// The seq of the transaction, or of the refs checked when nothing was
    /// committed.
    pub seq: Seq,
    /// Each spec's outcome, in order.
    pub statuses: Vec<(RefName, RefStatus)>,
    /// The pack added, when objects were missing from the repository.
    pub pack: Option<pb::PackRef>,
    /// Whether the transaction repeats an earlier commit of the same key.
    pub replayed: bool,
}

/// Errors of [`push_workspace`].
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// An object a new tip reaches is in neither the repository nor the
    /// workspace's packs (`object_missing`).
    #[error("object {0} is in neither the repository nor the workspace")]
    Missing(ObjectId),
    /// Reading objects failed.
    #[error(transparent)]
    Odb(#[from] OdbError),
    /// The assembled pack was refused (too large, or by the scanner) or
    /// could not be stored.
    #[error(transparent)]
    Pack(#[from] WsPackError),
    /// The ref log failed or refused the request itself.
    #[error(transparent)]
    Ref(#[from] RefError),
}

/// What a push needs of the repository's ref log: its latest state and a
/// commit, which the server routes to the repository's sequencer owner
/// (`RepoSequencers`, Task 14).
#[async_trait]
pub trait PushLog: Send + Sync {
    /// The latest refs (linearizable).
    async fn latest(&self) -> Result<Arc<RefSnapshot>, RefError>;
    /// Commits `txn`.
    async fn commit(&self, txn: RefTxn) -> Result<Receipt, RefError>;
}

#[async_trait]
impl<T: RefLog + ?Sized> PushLog for T {
    async fn latest(&self) -> Result<Arc<RefSnapshot>, RefError> {
        self.snapshot(ReadAt::Latest).await
    }

    async fn commit(&self, txn: RefTxn) -> Result<Receipt, RefError> {
        RefLog::commit(self, txn).await
    }
}

/// The objects reachable from `tips` that the repository lacks, as pack
/// entries: walked through `odb`, the repository with a workspace's packs
/// as its last layer (`WsOdb`, `objects::workspace_odb`). An object found in
/// the repository's layers is not descended into: the repository's objects
/// are connected (every push and save is checked). Stored entries that are
/// not deltas are copied as they are; deltas are resolved. Submodule
/// commits are not followed. The empty tree is added when a commit names it
/// and no pack holds it.
pub async fn push_closure(
    odb: &RangeOdb,
    tips: &[ObjectId],
) -> Result<BTreeMap<ObjectId, PackEntry>, PushError> {
    let hash = gix_hash::Kind::Sha1;
    let ws_layer = odb.layers().saturating_sub(1);
    let corrupt = |oid: ObjectId, err: &dyn std::fmt::Display| OdbError::CorruptObject {
        oid,
        reason: err.to_string(),
    };
    let mut out = BTreeMap::new();
    let mut seen: HashSet<ObjectId> = HashSet::new();
    let mut queue: VecDeque<ObjectId> = tips.iter().copied().collect();
    while let Some(oid) = queue.pop_front() {
        if !seen.insert(oid) {
            continue;
        }
        let Some(loc) = odb.locate(&oid).await? else {
            if oid == empty_tree() {
                out.insert(oid, PackEntry::Whole(Kind::Tree, Bytes::new()));
                continue;
            }
            return Err(PushError::Missing(oid));
        };
        if loc.layer != ws_layer {
            continue;
        }
        let header = odb.header(&loc).await?;
        let whole = if header.base.is_some() || header.kind != Kind::Blob {
            Some(odb.read_at(&loc).await?)
        } else {
            None
        };
        if let Some((kind, data)) = &whole {
            match kind {
                Kind::Blob => {}
                Kind::Commit => {
                    let commit = gix_object::CommitRef::from_bytes(data, hash)
                        .map_err(|err| corrupt(oid, &err))?;
                    queue.push_back(commit.tree());
                    queue.extend(commit.parents());
                }
                Kind::Tree => {
                    let tree = gix_object::TreeRef::from_bytes(data, hash)
                        .map_err(|err| corrupt(oid, &err))?;
                    for entry in &tree.entries {
                        if !entry.mode.is_commit() {
                            queue.push_back(entry.oid.to_owned());
                        }
                    }
                }
                Kind::Tag => {
                    let tag = gix_object::TagRef::from_bytes(data, hash)
                        .map_err(|err| corrupt(oid, &err))?;
                    queue.push_back(tag.target());
                }
            }
        }
        let entry = match (header.base, whole) {
            (Some(_), Some((kind, data))) => PackEntry::Whole(kind, data),
            _ => PackEntry::Raw(odb.raw_entry(&loc).await?),
        };
        out.insert(oid, entry);
    }
    Ok(out)
}

/// Whether `new` descends from `old` (or is it), searched through `odb`:
/// a walk of `new`'s ancestry that stops at commits older than `old` by
/// more than [`ANCESTRY_CLOCK_SKEW_SECS`]. `false` when `odb` does not hold
/// `old` or either is not a commit.
pub async fn is_fast_forward(
    odb: &RangeOdb,
    old: &ObjectId,
    new: &ObjectId,
) -> Result<bool, OdbError> {
    if old == new {
        return Ok(true);
    }
    let Some(old_time) = commit_of(odb, old).await?.map(|(_, time)| time) else {
        return Ok(false);
    };
    let floor = old_time.saturating_sub(ANCESTRY_CLOCK_SKEW_SECS);
    let mut seen: HashSet<ObjectId> = HashSet::new();
    let mut stack = vec![*new];
    while let Some(oid) = stack.pop() {
        if oid == *old {
            return Ok(true);
        }
        if !seen.insert(oid) {
            continue;
        }
        let Some((parents, time)) = commit_of(odb, &oid).await? else {
            continue;
        };
        if time < floor {
            continue;
        }
        stack.extend(parents);
    }
    Ok(false)
}

/// A commit's parents and committer time; `None` when `odb` does not hold
/// it or it is not a commit.
async fn commit_of(
    odb: &RangeOdb,
    oid: &ObjectId,
) -> Result<Option<(Vec<ObjectId>, i64)>, OdbError> {
    let (kind, data) = match odb.read(oid).await {
        Ok(found) => found,
        Err(OdbError::NotFound(_)) => return Ok(None),
        Err(err) => return Err(err),
    };
    if kind != Kind::Commit {
        return Ok(None);
    }
    let (_, parents, time) = crate::derived::graph::parse_commit(oid, &data)?;
    Ok(Some((parents, i64::try_from(time).unwrap_or(i64::MAX))))
}

/// One spec, checked against the latest refs.
enum Checked {
    Update(RefUpdate),
    Done(RefStatus),
}

/// Checks `spec` against `snapshot` (§6.3's rules, as the helper applies
/// them; ruling T27.3).
async fn check(
    odb: &RangeOdb,
    snapshot: &RefSnapshot,
    spec: &PushSpec,
) -> Result<Checked, OdbError> {
    let name = spec.dst.clone();
    let current = snapshot.refs.get(&name).copied();
    let update = |expect: Expect| {
        Checked::Update(RefUpdate {
            name: name.clone(),
            expect,
            new: spec.new,
        })
    };
    if let Some(lease) = spec.lease {
        let holds = match lease {
            Lease::Absent => current.is_none(),
            Lease::Oid(oid) => current == Some(oid),
        };
        if !holds {
            return Ok(Checked::Done(RefStatus::Stale { current }));
        }
        if current == spec.new {
            return Ok(Checked::Done(RefStatus::UpToDate));
        }
        let expect = match lease {
            Lease::Absent => Expect::Absent,
            Lease::Oid(oid) => Expect::Oid(oid),
        };
        // A leased update that is not a fast-forward is still a forced
        // one: protections against force refuse it here, since the state
        // machine sees only `Expect::Oid`.
        if let (Some(old), Some(new)) = (current, spec.new)
            && denies_force(snapshot, &name)
            && !is_fast_forward(odb, &old, &new).await?
        {
            return Ok(Checked::Done(RefStatus::Protected));
        }
        return Ok(update(expect));
    }
    let Some(new) = spec.new else {
        return Ok(match current {
            None => Checked::Done(RefStatus::NotFound),
            Some(_) if spec.force => update(Expect::Any),
            Some(old) => update(Expect::Oid(old)),
        });
    };
    let Some(old) = current else {
        return Ok(update(Expect::Absent));
    };
    if old == new {
        return Ok(Checked::Done(RefStatus::UpToDate));
    }
    let is_tag = name.as_str().starts_with("refs/tags/");
    let known = odb.contains(&old).await?;
    let ff = !is_tag && known && is_fast_forward(odb, &old, &new).await?;
    Ok(if ff {
        // A forced fast-forward is an ordinary update, so protections
        // against force do not refuse it.
        update(Expect::Oid(old))
    } else if spec.force {
        update(Expect::Any)
    } else if is_tag {
        Checked::Done(RefStatus::AlreadyExists)
    } else if !known {
        Checked::Done(RefStatus::FetchFirst)
    } else {
        Checked::Done(RefStatus::NonFastForward)
    })
}

fn denies_force(snapshot: &RefSnapshot, name: &RefName) -> bool {
    snapshot
        .protections
        .iter()
        .any(|rule| rule.deny_force && pattern_matches(&rule.pattern, name.as_str()))
}

/// The idempotency key of a push carrying the caller's `key`: bound to the
/// principal, so two callers' keys never meet (ruling T27.6).
pub fn push_key(principal: &Principal, key: &IdemKey) -> IdemKey {
    let mut hash = Sha256::new();
    hash.update(b"ws-push keyed\0");
    hash.update(principal.as_str().as_bytes());
    hash.update(b"\0");
    hash.update(key.as_str().as_bytes());
    keyed(hash)
}

/// §36 §4.5's derived key and request digest of one transaction without a
/// caller's key: the digest is SHA-256 over the pack checksum and the
/// sorted (name, expect, new) triples, and the key binds it to the
/// principal (as the helper's push).
fn derived_key(
    principal: &Principal,
    pack: Option<&str>,
    updates: &[RefUpdate],
) -> (IdemKey, [u8; 32]) {
    let mut sorted: Vec<&RefUpdate> = updates.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut body = Sha256::new();
    body.update(b"ws-push v1\0");
    body.update(pack.unwrap_or("").as_bytes());
    for update in sorted {
        body.update(b"\0");
        body.update(update.name.as_str().as_bytes());
        match update.expect {
            Expect::Oid(oid) => body.update(format!("\0oid:{oid}")),
            Expect::Absent => body.update(b"\0absent"),
            Expect::Any => body.update(b"\0any"),
        }
        match update.new {
            Some(oid) => body.update(format!("\0new:{oid}")),
            None => body.update(b"\0delete"),
        }
    }
    let digest: [u8; 32] = body.finalize().into();
    let mut hash = Sha256::new();
    hash.update(b"ws-push derived\0");
    hash.update(principal.as_str().as_bytes());
    hash.update(b"\0");
    hash.update(digest);
    (keyed(hash), digest)
}

fn keyed(hash: Sha256) -> IdemKey {
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    // 72 bytes of `[a-z0-9:-]`: within `IdemKey`'s rules.
    IdemKey::parse(&format!("ws-push:{hex}")).expect("a valid key")
}

/// Commits `txn`, repeating it with the same key while the outcome is lost
/// (`Unavailable`).
async fn commit(log: &dyn PushLog, txn: &RefTxn) -> Result<Receipt, RefError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match log.commit(txn.clone()).await {
            Err(RefError::Unavailable) if attempt < COMMIT_ATTEMPTS => {
                tokio::time::sleep(Duration::from_millis(50 * u64::from(attempt))).await;
            }
            other => return other,
        }
    }
}

/// Pushes the workspace objects `odb` reads (the repository with the
/// workspace's packs as its last layer) to the repository behind `log`,
/// storing the pack in `blobs` (the repository's pack store) after `scan`
/// passes every object. See the module docs.
pub async fn push_workspace(
    log: &dyn PushLog,
    odb: &RangeOdb,
    repo: &RepoId,
    blobs: &dyn BlobStore,
    scan: &dyn PackScan,
    req: &PushRequest,
) -> Result<Pushed, PushError> {
    let mut done: Vec<Option<RefStatus>> = vec![None; req.specs.len()];
    let mut pack: Option<Option<pb::PackRef>> = None;
    let finish =
        |done: Vec<Option<RefStatus>>, seq, pack: Option<Option<pb::PackRef>>, replayed| Pushed {
            seq,
            statuses: req
                .specs
                .iter()
                .zip(done)
                .map(|(spec, status)| {
                    (
                        spec.dst.clone(),
                        status.unwrap_or(RefStatus::Stale { current: None }),
                    )
                })
                .collect(),
            pack: pack.flatten(),
            replayed,
        };
    let mut last_seq = Seq(0);
    for round in 0..=MAX_FENCE_RETRIES {
        if round > 0 {
            let jitter = rand::rng().random_range(0..=10 * round as u64);
            tokio::time::sleep(Duration::from_millis(jitter)).await;
        }
        let snapshot = log.latest().await?;
        last_seq = snapshot.seq;
        let mut updates: Vec<(usize, RefUpdate)> = Vec::new();
        for (i, spec) in req.specs.iter().enumerate() {
            if done[i].is_some() {
                continue;
            }
            match check(odb, &snapshot, spec).await? {
                Checked::Update(update) => updates.push((i, update)),
                Checked::Done(status) => done[i] = Some(status),
            }
        }
        let refused = done.iter().flatten().any(|s| !s.is_ok());
        if req.atomic && refused {
            for status in &mut done {
                if status.is_none() {
                    *status = Some(RefStatus::AtomicFailed);
                }
            }
            return Ok(finish(done, snapshot.seq, pack, false));
        }
        if updates.is_empty() {
            return Ok(finish(done, snapshot.seq, pack, false));
        }
        if pack.is_none() {
            pack = Some(assemble(odb, repo, blobs, scan, &updates).await?);
        }
        let pack_ref = pack.clone().flatten();
        let group: Vec<RefUpdate> = updates.iter().map(|(_, u)| u.clone()).collect();
        let (key, request_digest) = match &req.key {
            Some((key, digest)) => (push_key(&req.principal, key), *digest),
            None => derived_key(
                &req.principal,
                pack_ref.as_ref().map(|p| p.checksum.as_str()),
                &group,
            ),
        };
        let txn = RefTxn {
            key,
            request_digest,
            updates: group,
            packs: pack_ref.iter().cloned().collect(),
            principal: req.principal.clone(),
            scope: None,
            trace: None,
            push_options: req.push_options.clone(),
        };
        match commit(log, &txn).await {
            Ok(receipt) => {
                for (i, update) in updates {
                    done[i] = Some(RefStatus::Updated {
                        old: snapshot.refs.get(&update.name).copied(),
                        new: update.new,
                    });
                }
                return Ok(finish(done, receipt.seq, pack, receipt.replayed));
            }
            Err(RefError::Rejected(rejected)) => {
                let reasons: BTreeMap<&RefName, &RejectReason> = rejected
                    .iter()
                    .map(|(name, reason)| (name, reason))
                    .collect();
                let mut final_refusal = false;
                for (i, update) in &updates {
                    let Some(reason) = reasons.get(&update.name) else {
                        continue;
                    };
                    let leased = req.specs[*i].lease.is_some();
                    done[*i] = match reason {
                        // Another push moved the ref: check it again,
                        // unless the caller's lease named where it was.
                        RejectReason::Stale { current } if leased => {
                            Some(RefStatus::Stale { current: *current })
                        }
                        RejectReason::Exists if leased => Some(RefStatus::Stale {
                            current: snapshot.refs.get(&update.name).copied(),
                        }),
                        RejectReason::Stale { .. } | RejectReason::Exists => None,
                        RejectReason::Protected => Some(RefStatus::Protected),
                        RejectReason::OutOfScope { path } => {
                            Some(RefStatus::OutOfScope { path: path.clone() })
                        }
                        RejectReason::InvalidName => Some(RefStatus::Invalid),
                    };
                    final_refusal |= done[*i].is_some();
                }
                if req.atomic && final_refusal {
                    for status in &mut done {
                        if status.is_none() {
                            *status = Some(RefStatus::AtomicFailed);
                        }
                    }
                    return Ok(finish(done, snapshot.seq, pack, false));
                }
            }
            Err(err) => return Err(err.into()),
        }
    }
    // Still racing after every retry: the remaining refs are stale.
    let latest = log.latest().await.ok();
    for (i, spec) in req.specs.iter().enumerate() {
        if done[i].is_none() {
            done[i] = Some(RefStatus::Stale {
                current: latest.as_ref().and_then(|s| s.refs.get(&spec.dst).copied()),
            });
        }
    }
    let seq = latest.map_or(last_seq, |s| s.seq);
    Ok(finish(done, seq, pack, false))
}

/// The pack of the objects the updates' new tips reach that the repository
/// lacks, verified, scanned and stored; `None` when it lacks none.
async fn assemble(
    odb: &RangeOdb,
    repo: &RepoId,
    blobs: &dyn BlobStore,
    scan: &dyn PackScan,
    updates: &[(usize, RefUpdate)],
) -> Result<Option<pb::PackRef>, PushError> {
    let mut tips: Vec<ObjectId> = updates.iter().filter_map(|(_, u)| u.new).collect();
    tips.sort();
    tips.dedup();
    let entries = push_closure(odb, &tips).await?;
    if entries.is_empty() {
        return Ok(None);
    }
    let (_, _, lpk) = write_pack(&entries).map_err(WsPackError::Blob)?;
    check_pack_len(lpk.len() as u64)?;
    let (verified, _) = put_workspace_pack(repo, blobs, lpk, scan).await?;
    Ok(Some(verified.pack))
}
