//! Committing and pushing workspaces served (GT1 Task 27, design §36 §19.7.5, D831):
//! `CommitWorkspace` and `PushWorkspace` on the main port.
//! `cargo test -p loams --features git --test git ws_push::`.
//!
//! The closure's minimality and the retry against a racing push are tested in `loams-git`
//! (`crates/loams-git/tests/ws_push.rs`), where a rival push can be put between the check and
//! the commit.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use loams_git::{BlobId, BlobStore, RepoPaths, StoreBlobStore, StoreWalStore};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::Running;
use super::objects::{git, history, load_ids, move_main, rev};
use super::workspaces::{Caller, WS, create_ws, n_of};

const REPOS: &str = "/loams.repos.v1.RepoService";

fn write(path: &str, data: &[u8]) -> Value {
    json!({"write": {"path": path, "content": STANDARD.encode(data)}})
}

/// A server holding `acme/web` (the history's `main`) and a workspace on it.
async fn setup() -> Option<(
    Running,
    TempDir,
    Caller,
    String,
    (loams_git::NamespaceId, loams_git::RepoId),
)> {
    let dir = history()?;
    let running = Running::start().await;
    let (_, ns, repo) = load_ids(&running, &dir).await;
    let op = Caller::of(&running, None);
    let ws = create_ws(&op, json!({"namespace": "acme", "name": "web"})).await["workspace"]
        .as_str()
        .expect("id")
        .to_owned();
    Some((running, dir, op, ws, (ns, repo)))
}

/// Edits (staged) on snapshot `n`, then a commit of them; the commit's snapshot.
async fn edit_and_commit(op: &Caller, ws: &str, n: u64, edits: Value, message: &str) -> Value {
    let applied = op
        .ok(
            &format!("{WS}/ApplyEdits"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws, "expectN": n,
                    "edits": edits, "stage": true}),
        )
        .await;
    assert_eq!(n_of(&applied["snapshot"]), n + 1);
    commit(op, ws, n + 1, message).await
}

async fn commit(op: &Caller, ws: &str, n: u64, message: &str) -> Value {
    op.ok(
        &format!("{WS}/CommitWorkspace"),
        &json!({"namespace": "acme", "name": "web", "workspace": ws, "expectN": n,
                "message": message}),
    )
    .await
}

fn push_body(ws: &str, updates: Value) -> Value {
    json!({"namespace": "acme", "name": "web", "workspace": ws, "updates": updates})
}

async fn main_of(op: &Caller) -> String {
    let refs = op
        .ok(
            &format!("{REPOS}/ListRefs"),
            &json!({"namespace": "acme", "name": "web"}),
        )
        .await;
    refs["refs"]
        .as_array()
        .expect("refs")
        .iter()
        .find(|r| r["name"] == "refs/heads/main")
        .and_then(|r| r["oid"].as_str())
        .expect("main")
        .to_owned()
}

/// Incompressible text, so the pushed pack's size shows the file was copied.
fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    (0..len)
        .map(|i| {
            if i % 64 == 63 {
                return b'\n';
            }
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            b"0123456789abcdef"[(state & 15) as usize]
        })
        .collect()
}

/// An agent edits, commits and pushes without uploading anything at push time: the push's
/// request is a few hundred bytes, the repository gains the 1 MiB file as one pack, `main`
/// moves to the commit, and the workspace's base follows, so a rebase sees no local commits.
/// The commit is the caller's, a retry of it answers the same commit, and a commit with
/// nothing staged is refused.
#[tokio::test]
async fn push_workspace_moves_no_client_bytes() {
    let Some((running, _dir, op, ws, _)) = setup().await else {
        return;
    };
    let big = noise(1 << 20);
    let committed = edit_and_commit(&op, &ws, 0, json!([write("big.txt", &big)]), "add big").await;
    let snap = &committed["snapshot"];
    assert_eq!(n_of(snap), 2);
    let commit_id = committed["commit"].as_str().expect("commit").to_owned();
    assert_eq!(snap["head"], commit_id);
    assert_ne!(snap["head"], snap["base"], "a local commit");
    let retried = commit(&op, &ws, 1, "add big").await;
    assert_eq!(
        retried["commit"], commit_id,
        "a retry answers the same commit"
    );
    let empty = op
        .refused(
            &format!("{WS}/CommitWorkspace"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws, "expectN": 2,
                    "message": "again"}),
        )
        .await;
    assert_eq!(empty, "failed_precondition");

    let body = push_body(&ws, json!([]));
    assert!(body.to_string().len() < 512, "the push uploads no objects");
    let pushed = op.ok(&format!("{WS}/PushWorkspace"), &body).await;
    let result = &pushed["results"][0];
    assert_eq!(result["refName"], "refs/heads/main");
    assert_eq!(result["status"], "PUSH_REF_STATUS_UPDATED", "{pushed}");
    assert_eq!(result["newOid"], commit_id);
    let pack_len: u64 = pushed["pack"]["packLen"].as_str().map_or_else(
        || pushed["pack"]["packLen"].as_u64().expect("len"),
        |s| s.parse().expect("len"),
    );
    assert!(pack_len > 400_000, "the big file was copied: {pack_len}");
    assert_eq!(main_of(&op).await, commit_id);
    assert!(
        pushed["consistencyToken"]
            .as_str()
            .is_some_and(|t| t.starts_with("git:")),
        "{pushed}"
    );
    let moved = &pushed["snapshot"];
    assert_eq!(n_of(moved), 3);
    assert_eq!(moved["base"], commit_id, "the base follows the push");
    // Nothing to rebase onto: main is the base.
    let rebased = op
        .ok(
            &format!("{WS}/Rebase"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws}),
        )
        .await;
    assert_eq!(n_of(&rebased["snapshot"]), 3);
    // Pushing again changes nothing.
    let again = op.ok(&format!("{WS}/PushWorkspace"), &body).await;
    assert_eq!(again["results"][0]["status"], "PUSH_REF_STATUS_UP_TO_DATE");
    assert!(again["pack"].is_null(), "{again}");
    running.shutdown().await;
}

/// Another writer moved `main`: a push of the workspace's commit is not a fast-forward and is
/// refused, a lease naming the old tip is stale, one naming the current tip moves it, and a
/// forced push moves it back.
#[tokio::test]
async fn push_non_ff_is_rejected_and_force_with_lease() {
    let Some((running, dir, op, ws, (ns, repo))) = setup().await else {
        return;
    };
    let old = rev(&dir, "main");
    let committed = edit_and_commit(&op, &ws, 0, json!([write("ws.txt", b"ws\n")]), "ws").await;
    let ours = committed["commit"].as_str().expect("commit").to_owned();
    std::fs::write(dir.path().join("other.txt"), "other\n").expect("write");
    git(dir.path(), &["add", "-A"], None);
    git(dir.path(), &["commit", "-q", "-m", "two"], None);
    move_main(&running, ns, &repo, &dir, Some(&old)).await;
    let theirs = rev(&dir, "main");

    let refused = op
        .ok(
            &format!("{WS}/PushWorkspace"),
            &push_body(&ws, json!([{"src": "HEAD", "dst": "main"}])),
        )
        .await;
    assert_eq!(
        refused["results"][0]["status"],
        "PUSH_REF_STATUS_NON_FAST_FORWARD"
    );
    assert_eq!(refused["results"][0]["message"], "non-fast-forward");
    assert!(refused["snapshot"].is_null(), "the base stays");
    let stale = op
        .ok(
            &format!("{WS}/PushWorkspace"),
            &push_body(&ws, json!([{"src": "HEAD", "dst": "main", "expect": old}])),
        )
        .await;
    assert_eq!(stale["results"][0]["status"], "PUSH_REF_STATUS_STALE");
    assert_eq!(stale["results"][0]["oldOid"], theirs);
    assert_eq!(main_of(&op).await, theirs);
    let leased = op
        .ok(
            &format!("{WS}/PushWorkspace"),
            &push_body(
                &ws,
                json!([{"src": "HEAD", "dst": "main", "expect": theirs}]),
            ),
        )
        .await;
    assert_eq!(
        leased["results"][0]["status"], "PUSH_REF_STATUS_UPDATED",
        "{leased}"
    );
    assert_eq!(main_of(&op).await, ours);
    let forced = op
        .ok(
            &format!("{WS}/PushWorkspace"),
            &push_body(
                &ws,
                json!([{"src": theirs, "dst": "refs/heads/main", "force": true}]),
            ),
        )
        .await;
    assert_eq!(
        forced["results"][0]["status"], "PUSH_REF_STATUS_UPDATED",
        "{forced}"
    );
    assert_eq!(main_of(&op).await, theirs);
    let unknown = op
        .refused(
            &format!("{WS}/PushWorkspace"),
            &push_body(&ws, json!([{"src": "nope", "dst": "main"}])),
        )
        .await;
    assert_eq!(unknown, "invalid_argument");
    running.shutdown().await;
}

/// A retry under the same key answers the same refs; the key with another request is
/// `idempotency_key_reused`.
#[tokio::test]
async fn push_is_idempotent_on_key() {
    let Some((running, _dir, op, ws, _)) = setup().await else {
        return;
    };
    edit_and_commit(&op, &ws, 0, json!([write("f.txt", b"f\n")]), "f").await;
    let mut body = push_body(&ws, json!([{"src": "HEAD", "dst": "refs/heads/feature"}]));
    body["idempotencyKey"] = json!("push-1");
    let first = op.ok(&format!("{WS}/PushWorkspace"), &body).await;
    assert_eq!(first["results"][0]["status"], "PUSH_REF_STATUS_UPDATED");
    let again = op.ok(&format!("{WS}/PushWorkspace"), &body).await;
    assert!(
        again["results"][0]["status"] == "PUSH_REF_STATUS_UP_TO_DATE",
        "{again}"
    );
    assert_eq!(again["seq"], first["seq"]);
    let mut other = push_body(&ws, json!([{"src": "HEAD", "dst": "refs/heads/other"}]));
    other["idempotencyKey"] = json!("push-1");
    let reused = op.refused(&format!("{WS}/PushWorkspace"), &other).await;
    assert_eq!(reused, "idempotency_key_reused");
    running.shutdown().await;
}

/// The pushed branch is a complete repository to stock git: its packs, copied out of the
/// bucket into a bare repository with the refs, clone and pass `git fsck`, and the clone has
/// the workspace's files.
#[tokio::test]
async fn pushed_branch_clones_with_stock_git() {
    let Some((running, _dir, op, ws, (ns, repo))) = setup().await else {
        return;
    };
    let committed = edit_and_commit(
        &op,
        &ws,
        0,
        json!([write("src/agent.rs", b"fn agent() {}\n"), {"delete": {"path": "README.md"}}]),
        "agent work",
    )
    .await;
    let commit_id = committed["commit"].as_str().expect("commit").to_owned();
    let pushed = op
        .ok(
            &format!("{WS}/PushWorkspace"),
            &push_body(&ws, json!([{"src": "HEAD", "dst": "refs/heads/agent"}])),
        )
        .await;
    assert_eq!(pushed["results"][0]["status"], "PUSH_REF_STATUS_UPDATED");
    assert!(
        pushed["snapshot"].is_null(),
        "the base moves only with track"
    );

    let store = running.bucket();
    let paths = RepoPaths::new(ns, &repo);
    let state = loams_git::latest_state(&StoreWalStore::new(store.clone(), paths.clone()))
        .await
        .expect("state")
        .snapshot();
    let blobs = Arc::new(StoreBlobStore::new(store, paths));
    let out = TempDir::new().expect("dir");
    let bare = out.path().join("bare.git");
    git(out.path(), &["init", "-q", "--bare", "bare.git"], None);
    for pack in &state.packs {
        let id = BlobId::parse(&pack.checksum).expect("id");
        let data = blobs.get_range(&id, 0..pack.pack_len).await.expect("pack");
        let idx = blobs
            .get_range(&id, pack.idx_offset..pack.idx_offset + pack.idx_len)
            .await
            .expect("idx");
        let name = bare.join(format!("objects/pack/pack-{}", pack.checksum));
        std::fs::write(name.with_extension("pack"), &data).expect("pack");
        std::fs::write(name.with_extension("idx"), &idx).expect("idx");
    }
    for (name, oid) in &state.refs {
        git(
            &bare,
            &["update-ref", name.as_str(), &oid.to_string()],
            None,
        );
    }
    git(&bare, &["fsck", "--full", "--no-dangling"], None);
    git(
        out.path(),
        &[
            "clone",
            "-q",
            "--no-local",
            "-b",
            "agent",
            "bare.git",
            "clone",
        ],
        None,
    );
    let clone = out.path().join("clone");
    assert_eq!(rev_in(&clone, "HEAD"), commit_id);
    assert_eq!(
        std::fs::read_to_string(clone.join("src/agent.rs")).expect("file"),
        "fn agent() {}\n"
    );
    assert!(!clone.join("README.md").exists());
    git(&clone, &["fsck", "--full"], None);
    running.shutdown().await;
}

fn rev_in(dir: &std::path::Path, name: &str) -> String {
    git(dir, &["rev-parse", name], None).trim().to_owned()
}
