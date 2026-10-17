//! Pushing workspaces (plan GT1 Task 27): the closure a push copies, the
//! checks it makes against the repository's refs, retries against racing
//! pushes, and the commits `CommitWorkspace` writes.

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use loams_git::format::pb;
use loams_git::odb::RangeOdb;
use loams_git::ws::{
    Ident, Lease, NoScan, PackEntry, PushLog, PushRequest, PushSpec, RefStatus, SERVER_COMMITTER,
    push_closure, push_workspace, same_workspace_commit, verify_workspace_pack, workspace_commit,
};
use loams_git::{
    BlobStore, BucketRefLog, Expect, IdemKey, ObjectId, Principal, ReadAt, Receipt, RefError,
    RefLog, RefName, RefSnapshot, RefTxn, RefUpdate, RepoId, StoreBlobStore, SystemClock, WsId,
    WsOdb,
};
use loams_store::Store;

use support::{GitRepo, faulty_store, oid, open_log, put_pack, repo_paths};

const MAIN: &str = "refs/heads/main";

fn main_ref() -> RefName {
    RefName::parse(MAIN).expect("ref")
}

fn principal() -> Principal {
    Principal::parse("agent-1").expect("principal")
}

/// A repository whose `main` is at `old`, with its objects in one pack.
struct Fixture {
    store: Store,
    repo: RepoId,
    blobs: StoreBlobStore,
    log: BucketRefLog,
    ws: WsId,
}

impl Fixture {
    async fn new(git: &GitRepo, old: &ObjectId) -> Self {
        let (_, store) = faulty_store();
        let (repo, paths) = repo_paths(27);
        let log = open_log(&store, &repo, Arc::new(SystemClock), |_| {}).await;
        let blobs = StoreBlobStore::new(store.clone(), paths);
        let pack = put_pack(&blobs, git.pack_revs(&[&old.to_string()], &[])).await;
        let fixture = Self {
            store,
            repo,
            blobs,
            log,
            ws: WsId::parse("w01j9z3m5q8x000000000000027").expect("ws"),
        };
        fixture
            .raw_commit(Expect::Absent, *old, vec![pack], "load")
            .await;
        fixture
    }

    /// Moves `main` directly, as another writer would.
    async fn raw_commit(&self, expect: Expect, new: ObjectId, packs: Vec<pb::PackRef>, key: &str) {
        RefLog::commit(
            &self.log,
            RefTxn {
                key: IdemKey::parse(key).expect("key"),
                request_digest: [7; 32],
                updates: vec![RefUpdate {
                    name: main_ref(),
                    expect,
                    new: Some(new),
                }],
                packs,
                principal: Principal::parse("other").expect("principal"),
                scope: None,
                trace: None,
                push_options: Vec::new(),
            },
        )
        .await
        .expect("commit");
    }

    fn ws_blobs(&self) -> StoreBlobStore {
        let (_, paths) = repo_paths(27);
        StoreBlobStore::workspace(self.store.clone(), &paths, &self.ws)
    }

    /// The repository at its latest seq with a workspace pack of `objects`
    /// searched last.
    async fn ws_odb(&self, git: &GitRepo, objects: &[ObjectId]) -> RangeOdb {
        let ws_blobs = self.ws_blobs();
        let ws_pack = put_pack(&ws_blobs, git.pack_objects(objects)).await;
        let snapshot = self.log.snapshot(ReadAt::Latest).await.expect("snapshot");
        let repo_odb = RangeOdb::open(
            self.repo.clone(),
            Arc::new(self.blobs.clone()),
            &snapshot,
            None,
        )
        .await
        .expect("open");
        WsOdb::open(&repo_odb, Arc::new(ws_blobs), &[ws_pack])
            .expect("ws odb")
            .into_odb()
    }

    async fn main(&self) -> Option<ObjectId> {
        let snapshot = self.log.snapshot(ReadAt::Latest).await.expect("snapshot");
        snapshot.refs.get(&main_ref()).copied()
    }
}

fn request(new: ObjectId, force: bool, lease: Option<Lease>) -> PushRequest {
    PushRequest {
        specs: vec![PushSpec {
            dst: main_ref(),
            new: Some(new),
            force,
            lease,
        }],
        atomic: false,
        principal: principal(),
        push_options: Vec::new(),
        key: None,
    }
}

/// `old` (main), then `c2` and `c3` on top, sharing most of their tree.
fn history(git: &GitRepo) -> (ObjectId, ObjectId, ObjectId) {
    let body: String = (0..200).map(|i| format!("line {i}\n")).collect();
    let old = git.commit(
        &[
            ("a.txt", &body),
            ("src/lib.rs", "// lib\n"),
            ("d/e/f", "f\n"),
        ],
        "old",
    );
    let changed = format!("{body}one more\n");
    let c2 = git.commit(&[("a.txt", &changed), ("src/new.rs", "new\n")], "c2");
    let again = format!("{changed}two more\n");
    let c3 = git.commit(
        &[("d/e/g", "g\n"), ("b.txt", &changed), ("a.txt", &again)],
        "c3",
    );
    (old, c2, c3)
}

/// The closure is what `git rev-list --objects new --not old` lists, even
/// when the workspace pack also holds the repository's objects and an
/// unreachable one; stored entries are copied, deltas resolved, and the
/// pack written from it verifies as an upload does.
#[tokio::test]
async fn push_closure_is_minimal() {
    let Some(git) = GitRepo::init("push_closure_is_minimal") else {
        return;
    };
    let (old, _, c3) = history(&git);
    let fixture = Fixture::new(&git, &old).await;
    let stray = oid(String::from_utf8(
        git.git(&["hash-object", "-w", "--stdin"], Some(b"stray\n")),
    )
    .expect("utf-8")
    .trim());
    let mut objects = git.objects(&[&c3.to_string()]);
    objects.push(stray);
    let odb = fixture.ws_odb(&git, &objects).await;
    let closure = push_closure(&odb, &[c3]).await.expect("closure");
    let got: BTreeSet<ObjectId> = closure.keys().copied().collect();
    let want: BTreeSet<ObjectId> = git
        .objects(&[&c3.to_string(), "--not", &old.to_string()])
        .into_iter()
        .collect();
    assert_eq!(got, want);
    assert!(
        closure.values().any(|e| matches!(e, PackEntry::Raw(_))),
        "stored entries are copied"
    );
    // git stores c2's `a.txt` as a delta against c3's: that entry is
    // resolved.
    assert!(
        closure.values().any(|e| matches!(e, PackEntry::Whole(..))),
        "deltas are resolved"
    );
    let pushed = push_workspace(
        &fixture.log,
        &odb,
        &fixture.repo,
        &fixture.blobs,
        &NoScan,
        &request(c3, false, None),
    )
    .await
    .expect("push");
    assert_eq!(
        pushed.statuses[0].1,
        RefStatus::Updated {
            old: Some(old),
            new: Some(c3)
        }
    );
    let pack = pushed.pack.expect("a pack");
    assert_eq!(pack.object_count as usize, want.len());
    let id = loams_git::BlobId::parse(&pack.checksum).expect("id");
    let len = fixture
        .blobs
        .head(&id)
        .await
        .expect("head")
        .expect("stored");
    let lpk = fixture.blobs.get_range(&id, 0..len).await.expect("stored");
    let verified = verify_workspace_pack(&fixture.repo, &lpk, &NoScan)
        .await
        .expect("verifies");
    assert_eq!(
        verified.objects.into_iter().collect::<BTreeSet<_>>(),
        want,
        "the stored pack holds the closure"
    );
    assert_eq!(fixture.main().await, Some(c3));
}

/// A push of a commit that does not descend from `main` is refused unless
/// forced; a lease that holds moves it, one that does not is stale; an
/// existing tag is not moved.
#[tokio::test]
async fn push_non_ff_force_and_lease() {
    let Some(git) = GitRepo::init("push_non_ff_force_and_lease") else {
        return;
    };
    let (old, c2, _) = history(&git);
    git.git(&["checkout", "-q", "-b", "side", &old.to_string()], None);
    let side = git.commit(&[("side.txt", "side\n")], "side");
    let fixture = Fixture::new(&git, &old).await;
    fixture
        .raw_commit(
            Expect::Oid(old),
            c2,
            vec![
                put_pack(
                    &fixture.blobs,
                    git.pack_revs(&[&c2.to_string(), &format!("^{old}")], &[]),
                )
                .await,
            ],
            "c2",
        )
        .await;
    let odb = fixture
        .ws_odb(
            &git,
            &git.objects(&[&side.to_string(), "--not", &old.to_string()]),
        )
        .await;
    let push = |req: PushRequest| {
        let fixture = &fixture;
        let odb = &odb;
        async move {
            push_workspace(
                &fixture.log,
                odb,
                &fixture.repo,
                &fixture.blobs,
                &NoScan,
                &req,
            )
            .await
            .expect("push")
        }
    };
    let refused = push(request(side, false, None)).await;
    assert_eq!(refused.statuses[0].1, RefStatus::NonFastForward);
    assert_eq!(refused.pack, None, "a refused push stores nothing");
    let stale = push(request(side, false, Some(Lease::Oid(old)))).await;
    assert_eq!(stale.statuses[0].1, RefStatus::Stale { current: Some(c2) });
    let leased = push(request(side, false, Some(Lease::Oid(c2)))).await;
    assert_eq!(
        leased.statuses[0].1,
        RefStatus::Updated {
            old: Some(c2),
            new: Some(side)
        }
    );
    assert_eq!(fixture.main().await, Some(side));
    let forced = push(request(old, true, None)).await;
    assert!(forced.statuses[0].1.is_ok(), "{forced:?}");
    let again = push(request(old, false, None)).await;
    assert_eq!(again.statuses[0].1, RefStatus::UpToDate);
    let tag = RefName::parse("refs/tags/v1").expect("tag");
    let mut tagged = request(side, false, None);
    tagged.specs[0].dst = tag.clone();
    assert!(push(tagged.clone()).await.statuses[0].1.is_ok());
    tagged.specs[0].new = Some(old);
    assert_eq!(push(tagged).await.statuses[0].1, RefStatus::AlreadyExists);
    // Atomic: a refused ref keeps the other from moving.
    let feature = RefName::parse("refs/heads/feature").expect("ref");
    let mut atomic = request(side, false, None);
    atomic.specs.push(PushSpec {
        dst: feature.clone(),
        new: Some(side),
        force: false,
        lease: None,
    });
    atomic.specs[0].dst = tag;
    atomic.specs[0].new = Some(old);
    atomic.atomic = true;
    let refused = push(atomic).await;
    assert_eq!(refused.statuses[0].1, RefStatus::AlreadyExists);
    assert_eq!(refused.statuses[1].1, RefStatus::AtomicFailed);
    let snapshot = fixture
        .log
        .snapshot(ReadAt::Latest)
        .await
        .expect("snapshot");
    assert!(!snapshot.refs.contains_key(&feature), "nothing moved");
}

/// A ref log that lets another push move `main` before the first commit
/// of this push reaches it.
struct Racing<'a> {
    fixture: &'a Fixture,
    rival: (ObjectId, ObjectId, Vec<pb::PackRef>),
    raced: AtomicBool,
}

#[async_trait]
impl PushLog for Racing<'_> {
    async fn latest(&self) -> Result<Arc<RefSnapshot>, RefError> {
        self.fixture.log.snapshot(ReadAt::Latest).await
    }

    async fn commit(&self, txn: RefTxn) -> Result<Receipt, RefError> {
        if !self.raced.swap(true, Ordering::SeqCst) {
            let (from, to, packs) = self.rival.clone();
            self.fixture
                .raw_commit(Expect::Oid(from), to, packs, "rival")
                .await;
        }
        RefLog::commit(&self.fixture.log, txn).await
    }
}

/// When another push moves `main` between the check and the commit, the
/// transaction is refused, the update is checked again against the new
/// tip, and committed when it still fast-forwards; a rival tip the
/// workspace does not have makes it `FetchFirst`.
#[tokio::test]
async fn push_racing_another_push_fences_and_revalidates() {
    let Some(git) = GitRepo::init("push_racing_another_push") else {
        return;
    };
    let (old, c2, c3) = history(&git);
    git.git(&["checkout", "-q", "-b", "side", &old.to_string()], None);
    let side = git.commit(&[("side.txt", "side\n")], "side");
    for (rival, want) in [
        (
            c2,
            RefStatus::Updated {
                old: Some(c2),
                new: Some(c3),
            },
        ),
        (side, RefStatus::FetchFirst),
    ] {
        let fixture = Fixture::new(&git, &old).await;
        let rival_pack = put_pack(
            &fixture.blobs,
            git.pack_revs(&[&rival.to_string(), &format!("^{old}")], &[]),
        )
        .await;
        let odb = fixture
            .ws_odb(
                &git,
                &git.objects(&[&c3.to_string(), "--not", &old.to_string()]),
            )
            .await;
        let racing = Racing {
            fixture: &fixture,
            rival: (old, rival, vec![rival_pack]),
            raced: AtomicBool::new(false),
        };
        let pushed = push_workspace(
            &racing,
            &odb,
            &fixture.repo,
            &fixture.blobs,
            &NoScan,
            &request(c3, false, None),
        )
        .await
        .expect("push");
        assert_eq!(pushed.statuses[0].1, want, "rival {rival}");
        let expected_main = if want.is_ok() { c3 } else { rival };
        assert_eq!(fixture.main().await, Some(expected_main));
    }
}

/// A retry under the caller's key replays; the key with another request is
/// refused.
#[tokio::test]
async fn push_is_idempotent_on_key() {
    let Some(git) = GitRepo::init("push_is_idempotent_on_key") else {
        return;
    };
    let (old, c2, c3) = history(&git);
    let fixture = Fixture::new(&git, &old).await;
    let odb = fixture
        .ws_odb(
            &git,
            &git.objects(&[&c3.to_string(), "--not", &old.to_string()]),
        )
        .await;
    let key = IdemKey::parse("push-1").expect("key");
    let mut req = request(c2, false, None);
    req.key = Some((key.clone(), [1; 32]));
    let first = push_workspace(
        &fixture.log,
        &odb,
        &fixture.repo,
        &fixture.blobs,
        &NoScan,
        &req,
    )
    .await
    .expect("push");
    assert!(first.statuses[0].1.is_ok());
    // The same request again: the ref is where it asked.
    let again = push_workspace(
        &fixture.log,
        &odb,
        &fixture.repo,
        &fixture.blobs,
        &NoScan,
        &req,
    )
    .await
    .expect("push");
    assert_eq!(again.statuses[0].1, RefStatus::UpToDate);
    assert_eq!(again.seq, first.seq);
    // Another request under the key.
    let mut other = request(c3, false, None);
    other.key = Some((key, [2; 32]));
    let err = push_workspace(
        &fixture.log,
        &odb,
        &fixture.repo,
        &fixture.blobs,
        &NoScan,
        &other,
    )
    .await
    .expect_err("key reused");
    assert!(
        matches!(
            err,
            loams_git::ws::PushError::Ref(RefError::IdempotencyMismatch)
        ),
        "{err:?}"
    );
}

/// A workspace commit hashes as git hashes it, and a retry's check knows
/// it.
#[test]
fn workspace_commit_matches_git() {
    let Some(git) = GitRepo::init("workspace_commit_matches_git") else {
        return;
    };
    let base = git.commit(&[("a", "a\n")], "base");
    let tree = git.rev("HEAD^{tree}");
    let author = Ident::of_principal("user-<1>", "Ada <Lovelace>", None);
    assert_eq!(author.line(), "Ada Lovelace <user-1@loams.invalid>");
    let (id, body) = workspace_commit(
        &tree,
        Some(&base),
        &author,
        &SERVER_COMMITTER,
        1_792_195_200,
        "fix it",
    )
    .expect("commit");
    let stored = String::from_utf8(git.git(
        &["hash-object", "-t", "commit", "-w", "--stdin"],
        Some(&body),
    ))
    .expect("utf-8");
    assert_eq!(stored.trim(), id.to_string());
    git.git(&["fsck", "--strict", "--no-dangling"], None);
    assert!(same_workspace_commit(
        &body,
        &tree,
        Some(&base),
        &author,
        "fix it\n"
    ));
    assert!(!same_workspace_commit(
        &body, &tree, None, &author, "fix it"
    ));
    assert!(!same_workspace_commit(
        &body,
        &tree,
        Some(&base),
        &author,
        "other"
    ));
}
