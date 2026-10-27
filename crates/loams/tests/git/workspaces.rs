//! `WorkspaceService` served (GT1 Task 25, design §36 §19.4, §19.10): workspaces on the bucket,
//! their snapshots and salvage, who may read and write them, and the object reads and uploads
//! that name them. `cargo test -p loams --features git --test git workspaces::`.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use loams::auth::{AuthConfig, Authenticators};
use loams_git::LpkWriter;
use loams_git::format::pb as gitpb;
use loams_git::format::snapshot::snapshot_commit;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::console_support::{Client, SETUP_TOKEN, TestAuthenticator};
use super::objects::{history, load, rev};
use super::{Running, frames, reason};

pub(crate) const WS: &str = "/loams.repos.v1.WorkspaceService";
pub(crate) const OBJECTS: &str = "/loams.repos.v1.ObjectService";
/// Every save's time: whole seconds, as the snapshot commit keeps them.
const SAVED_AT: &str = "2026-10-17T00:00:00Z";
const SAVED_AT_MS: i64 = 1_792_195_200_000;

/// A caller of the main port: no credential (the operator on loopback) or a bearer.
#[derive(Clone)]
pub(crate) struct Caller {
    base: String,
    bearer: Option<String>,
}

impl Caller {
    pub(crate) fn of(running: &Running, bearer: Option<&str>) -> Self {
        Self {
            base: running.base.clone(),
            bearer: bearer.map(str::to_owned),
        }
    }

    fn post(&self, rpc: &str, content_type: &str, body: Vec<u8>) -> reqwest::RequestBuilder {
        let mut request = reqwest::Client::new()
            .post(format!("{}{rpc}", self.base))
            .header("content-type", content_type)
            .body(body);
        if let Some(bearer) = &self.bearer {
            request = request.header("authorization", format!("Bearer {bearer}"));
        }
        request
    }

    pub(crate) async fn call(&self, rpc: &str, body: &Value) -> (reqwest::StatusCode, Value) {
        let response = self
            .post(rpc, "application/json", body.to_string().into_bytes())
            .send()
            .await
            .expect("call");
        let status = response.status();
        let text = response.text().await.expect("a body");
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    pub(crate) async fn ok(&self, rpc: &str, body: &Value) -> Value {
        let (status, answer) = self.call(rpc, body).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{rpc}: {answer}");
        answer
    }

    /// The `ErrorInfo.reason` of a refused unary call.
    pub(crate) async fn refused(&self, rpc: &str, body: &Value) -> String {
        let (status, answer) = self.call(rpc, body).await;
        assert_ne!(status, reqwest::StatusCode::OK, "{rpc}: {answer}");
        reason(&answer)
    }

    /// A streaming call (one request message), read to its end: the messages and the end
    /// message's error, if any.
    pub(crate) async fn stream(&self, rpc: &str, body: &Value) -> (Vec<Value>, Value) {
        let payload = body.to_string().into_bytes();
        let mut framed = vec![0u8];
        framed.extend_from_slice(&u32::try_from(payload.len()).expect("small").to_be_bytes());
        framed.extend_from_slice(&payload);
        let response = self
            .post(rpc, "application/connect+json", framed)
            .send()
            .await
            .expect("stream call");
        let bytes = response.bytes().await.expect("a body");
        let mut messages = Vec::new();
        let mut end = Value::Null;
        for (flags, data) in frames(&bytes) {
            let value: Value = serde_json::from_slice(data).unwrap_or(Value::Null);
            if flags & 0x02 != 0 {
                end = value["error"].clone();
            } else {
                messages.push(value);
            }
        }
        (messages, end)
    }

    /// `PutWorkspacePack` of one bundle; the stored `PackRef` as JSON, or the refusal.
    async fn put_pack(&self, workspace: &str, lpk: &[u8]) -> Result<Value, Value> {
        let (messages, end) = self
            .stream(
                &format!("{OBJECTS}/PutWorkspacePack"),
                &json!({"namespace": "acme", "name": "web", "workspace": workspace,
                        "data": STANDARD.encode(lpk)}),
            )
            .await;
        if end.is_null() {
            Ok(messages[0]["pack"].clone())
        } else {
            Err(end)
        }
    }
}

/// Runs git in `dir` with `env`; its stdout, trimmed.
pub(crate) fn git_env(
    dir: &std::path::Path,
    env: &[(&str, &str)],
    args: &[&str],
    stdin: &[u8],
) -> String {
    let mut child = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("git runs");
    {
        use std::io::Write as _;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stdin)
            .expect("stdin");
    }
    let out = child.wait_with_output().expect("git");
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .trim()
        .to_owned()
}

/// The tree of `main` with `files` written (and `removed` gone), built in a scratch index of
/// the history checkout; its id.
pub(crate) fn tree_with(dir: &TempDir, files: &[(&str, &str)], removed: &[&str]) -> String {
    let index = dir.path().join(".git/ws-index");
    let _ = std::fs::remove_file(&index);
    let index = index.to_str().expect("utf-8").to_owned();
    let env = [("GIT_INDEX_FILE", index.as_str())];
    git_env(dir.path(), &env, &["read-tree", "main"], b"");
    for (path, text) in files {
        let blob = git_env(
            dir.path(),
            &[],
            &["hash-object", "-w", "--stdin"],
            text.as_bytes(),
        );
        git_env(
            dir.path(),
            &env,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{blob},{path}"),
            ],
            b"",
        );
    }
    for path in removed {
        git_env(
            dir.path(),
            &env,
            &["update-index", "--force-remove", path],
            b"",
        );
    }
    git_env(dir.path(), &env, &["write-tree"], b"")
}

fn bytes(hex: &str) -> Vec<u8> {
    if hex.is_empty() {
        return Vec::new();
    }
    loams_git::ObjectId::from_hex(hex.as_bytes())
        .expect("hex")
        .as_bytes()
        .to_vec()
}

/// One save's objects, written: the snapshot commit of `ws@n` (tree `tree`, parent `head`)
/// stored in the checkout, then every object of `tree` and the commit as a bundle; the
/// commit's id and the bundle.
fn save_objects(dir: &TempDir, ws: &str, n: u64, tree: &str, head: &str) -> (String, Vec<u8>) {
    let snap = gitpb::WorkspaceSnapshot {
        ws_id: ws.to_owned(),
        n,
        worktree_tree: bytes(tree),
        head: bytes(head),
        created_unix_ms: SAVED_AT_MS,
        ..Default::default()
    };
    let (commit, body) = snapshot_commit(&snap).expect("a snapshot commit");
    let stored = git_env(
        dir.path(),
        &[],
        &["hash-object", "-t", "commit", "-w", "--stdin"],
        &body,
    );
    assert_eq!(stored, commit.to_string(), "git agrees on the commit");
    let mut objects = git_env(dir.path(), &[], &["ls-tree", "-r", "-t", tree], b"")
        .lines()
        .map(|line| {
            line.split_whitespace()
                .nth(2)
                .expect("an object")
                .to_owned()
        })
        .collect::<Vec<_>>();
    objects.push(tree.to_owned());
    objects.push(stored.clone());
    let list = objects.join("\n") + "\n";
    let name = git_env(
        dir.path(),
        &[],
        &["pack-objects", "-q", "ws-out"],
        list.as_bytes(),
    );
    let pack = std::fs::read(dir.path().join(format!("ws-out-{name}.pack"))).expect("pack");
    let idx = std::fs::read(dir.path().join(format!("ws-out-{name}.idx"))).expect("idx");
    let (_, _, lpk) = LpkWriter::from_parts(pack.into(), idx.into()).expect("bundle");
    (stored, lpk.to_vec())
}

/// The request body of a save of `ws@n` with these trees, head, packs and writer.
fn save_body(
    ws: &str,
    n: u64,
    tree: &str,
    head: &str,
    commit: &str,
    packs: &[Value],
    writer: &str,
) -> Value {
    json!({"namespace": "acme", "name": "web", "workspace": ws, "snapshot": {
        "n": n, "track": "refs/heads/main", "head": head, "headRef": "refs/heads/main",
        "indexTree": tree, "worktreeTree": tree, "snapshotCommit": commit,
        "packs": packs, "writer": writer, "createdAt": SAVED_AT,
    }})
}

/// Uploads a save's objects and saves `ws@n`; the answer.
async fn save(
    caller: &Caller,
    dir: &TempDir,
    ws: &str,
    n: u64,
    tree: &str,
    writer: &str,
) -> (reqwest::StatusCode, Value) {
    let head = rev(dir, "main");
    let (commit, lpk) = save_objects(dir, ws, n, tree, &head);
    let pack = caller.put_pack(ws, &lpk).await.expect("the pack is stored");
    caller
        .call(
            &format!("{WS}/SaveSnapshot"),
            &save_body(ws, n, tree, &head, &commit, &[pack], writer),
        )
        .await
}

pub(crate) fn mount(n: u8) -> String {
    format!("m01j9z3m5q8x0000000000000{n:02}")
}

pub(crate) async fn create_ws(caller: &Caller, body: Value) -> Value {
    caller.ok(&format!("{WS}/CreateWorkspace"), &body).await["snapshot"].clone()
}

pub(crate) fn n_of(snapshot: &Value) -> u64 {
    match &snapshot["n"] {
        Value::String(s) => s.parse().expect("n"),
        Value::Number(n) => n.as_u64().expect("n"),
        _ => 0,
    }
}

#[tokio::test]
async fn create_get_list_delete_round_trip() {
    let Some(dir) = history() else {
        return;
    };
    let running = Running::start().await;
    load(&running, &dir).await;
    let op = Caller::of(&running, None);
    let main = rev(&dir, "main");
    let named = json!({"namespace": "acme", "name": "web", "workspaceName": "feat",
                       "idempotencyKey": "c1"});
    let zero = create_ws(&op, named.clone()).await;
    let ws = zero["workspace"].as_str().expect("an id").to_owned();
    assert!(ws.starts_with('w') && ws.len() == 27, "{ws}");
    assert_eq!(n_of(&zero), 0);
    assert_eq!(zero["owner"], "dev-loopback");
    assert_eq!(zero["workspaceName"], "feat");
    assert_eq!(zero["track"], "refs/heads/main");
    assert_eq!(zero["base"], main);
    assert_eq!(zero["head"], main);
    assert_eq!(zero["headRef"], "refs/heads/main");
    assert_eq!(zero["worktreeTree"], rev(&dir, "main^{tree}"));
    assert_eq!(zero["indexTree"], zero["worktreeTree"]);
    // A retry answers the same workspace; the key with another request is refused, and the
    // name under another key is taken.
    assert_eq!(create_ws(&op, named).await["workspace"], ws.as_str());
    let rpc = format!("{WS}/CreateWorkspace");
    assert_eq!(
        op.refused(
            &rpc,
            &json!({"namespace": "acme", "name": "web", "workspaceName": "other",
                    "idempotencyKey": "c1"})
        )
        .await,
        "idempotency_key_reused"
    );
    assert_eq!(
        op.refused(
            &rpc,
            &json!({"namespace": "acme", "name": "web", "workspaceName": "feat",
                    "idempotencyKey": "c2"})
        )
        .await,
        "workspace_name_taken"
    );
    // An unnamed workspace at a commit.
    let unnamed = create_ws(&op, json!({"namespace": "acme", "name": "web", "at": main})).await;
    let other = unnamed["workspace"].as_str().expect("an id").to_owned();
    assert_eq!(unnamed["workspaceName"].as_str().unwrap_or(""), "");
    assert_eq!(
        op.refused(
            &rpc,
            &json!({"namespace": "acme", "name": "web",
                    "at": "00000000000000000000000000000000deadbeef"})
        )
        .await,
        "object_missing"
    );

    let got = op
        .ok(
            &format!("{WS}/GetWorkspace"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws}),
        )
        .await;
    assert_eq!(got["snapshot"], zero);
    let mine = op
        .ok(
            &format!("{WS}/ListWorkspaces"),
            &json!({"namespace": "acme", "name": "web", "owner": "dev-loopback"}),
        )
        .await;
    assert_eq!(mine["workspaces"][0]["workspace"], ws.as_str(), "{mine}");
    assert_eq!(mine["workspaces"][0]["workspaceName"], "feat");
    assert_eq!(mine["workspaces"].as_array().expect("a list").len(), 1);
    let mut all = Vec::new();
    let mut token = String::new();
    loop {
        let page = op
            .ok(
                &format!("{WS}/ListWorkspaces"),
                &json!({"namespace": "acme", "name": "web", "pageSize": 1, "pageToken": token}),
            )
            .await;
        all.extend(
            page["workspaces"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|w| w["workspace"].as_str().expect("id").to_owned()),
        );
        token = page["nextPageToken"].as_str().unwrap_or("").to_owned();
        if token.is_empty() {
            break;
        }
    }
    let mut want = vec![ws.clone(), other.clone()];
    want.sort();
    assert_eq!(all, want);
    let snaps = op
        .ok(
            &format!("{WS}/ListSnapshots"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws}),
        )
        .await;
    assert_eq!(snaps["snapshots"].as_array().expect("a list").len(), 1);
    assert_eq!(
        snaps["snapshots"][0]["snapshotCommit"],
        zero["snapshotCommit"]
    );
    let first = op
        .ok(
            &format!("{WS}/GetSnapshot"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws, "n": 0}),
        )
        .await;
    assert_eq!(first["snapshot"], zero);
    assert_eq!(
        op.refused(
            &format!("{WS}/GetSnapshot"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws, "n": 9}),
        )
        .await,
        "workspace_not_found"
    );

    let deleted = op
        .ok(
            &format!("{WS}/DeleteWorkspace"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws, "idempotencyKey": "d1"}),
        )
        .await;
    assert_eq!(deleted["snapshot"]["label"], "deleted");
    assert_eq!(n_of(&deleted["snapshot"]), 1);
    running.shutdown().await;
}

#[tokio::test]
async fn delete_writes_deleted_label() {
    let Some(dir) = history() else {
        return;
    };
    let running = Running::start().await;
    load(&running, &dir).await;
    let op = Caller::of(&running, None);
    let ws = create_ws(&op, json!({"namespace": "acme", "name": "web"})).await["workspace"]
        .as_str()
        .expect("id")
        .to_owned();
    let body = json!({"namespace": "acme", "name": "web", "workspace": ws});
    let deleted = op.ok(&format!("{WS}/DeleteWorkspace"), &body).await;
    assert_eq!(deleted["snapshot"]["label"], "deleted");
    // The label is the final snapshot, stored as any other.
    let store = running.bucket();
    let suffix = format!("/ws/{ws}/snap/00000000000000000001.lws");
    let listed = store.list_page("ns/", None, 10_000).await.expect("list");
    let path = listed
        .iter()
        .map(|info| info.path.clone())
        .find(|path| path.ends_with(&suffix))
        .expect("snapshot 1 is stored");
    let (raw, _) = store.get(&path).await.expect("read");
    let last = loams_git::format::snapshot::decode_snapshot(&raw).expect("a snapshot");
    assert_eq!(last.label, "deleted");
    // Deleting again answers the same; the workspace is gone for every other RPC.
    let again = op.ok(&format!("{WS}/DeleteWorkspace"), &body).await;
    assert_eq!(again, deleted);
    for rpc in [
        "GetWorkspace",
        "ListSnapshots",
        "ListSalvage",
        "WatchWorkspace",
    ] {
        let reason = if rpc == "WatchWorkspace" {
            let (_, end) = op.stream(&format!("{WS}/{rpc}"), &body).await;
            super::reason(&end)
        } else {
            op.refused(&format!("{WS}/{rpc}"), &body).await
        };
        assert_eq!(reason, "workspace_not_found", "{rpc}");
    }
    assert_eq!(
        op.refused(
            &format!("{OBJECTS}/HasObjects"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws,
                    "oids": [rev(&dir, "main")]}),
        )
        .await,
        "workspace_not_found"
    );
    running.shutdown().await;
}

#[tokio::test]
async fn save_snapshot_fenced_on_n() {
    let Some(dir) = history() else {
        return;
    };
    let running = Running::start().await;
    load(&running, &dir).await;
    let op = Caller::of(&running, None);
    let ws = create_ws(&op, json!({"namespace": "acme", "name": "web"})).await["workspace"]
        .as_str()
        .expect("id")
        .to_owned();
    let tree = tree_with(&dir, &[("notes/new.txt", "saved\n")], &[]);
    let (status, saved) = save(&op, &dir, &ws, 1, &tree, &mount(1)).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{saved}");
    assert_eq!(n_of(&saved["snapshot"]), 1);
    assert_eq!(saved["snapshot"]["worktreeTree"], tree.as_str());
    assert_eq!(saved["snapshot"]["principal"], "dev-loopback");
    assert_eq!(
        saved["snapshot"]["packs"].as_array().expect("packs").len(),
        1
    );

    // The same save again is answered as saved: it is the stored snapshot.
    let head = rev(&dir, "main");
    let (commit, _) = save_objects(&dir, &ws, 1, &tree, &head);
    let pack = saved["snapshot"]["packs"][0].clone();
    let again = op
        .ok(
            &format!("{WS}/SaveSnapshot"),
            &save_body(
                &ws,
                1,
                &tree,
                &head,
                &commit,
                std::slice::from_ref(&pack),
                &mount(1),
            ),
        )
        .await;
    assert_eq!(again["snapshot"], saved["snapshot"]);

    // Another writer offering snapshot 1 is fenced, and its state is kept as salvage.
    let (status, error) = op
        .call(
            &format!("{WS}/SaveSnapshot"),
            &save_body(
                &ws,
                1,
                &tree,
                &head,
                &commit,
                std::slice::from_ref(&pack),
                &mount(2),
            ),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::CONFLICT, "{error}");
    assert_eq!(reason(&error), "workspace_fenced");
    let salvage = op
        .ok(
            &format!("{WS}/ListSalvage"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws}),
        )
        .await;
    assert_eq!(salvage["salvage"][0]["mount"], mount(2).as_str());
    assert_eq!(n_of(&salvage["salvage"][0]), 1);

    // A save past the next number, a wrong snapshot commit and the `deleted` label are refused.
    for (n, commit, label) in [
        (3, commit.as_str(), ""),
        (2, "00000000000000000000000000000000deadbeef", ""),
        (2, commit.as_str(), "deleted"),
    ] {
        let mut body = save_body(
            &ws,
            n,
            &tree,
            &head,
            commit,
            std::slice::from_ref(&pack),
            &mount(1),
        );
        body["snapshot"]["label"] = json!(label);
        assert_eq!(
            op.refused(&format!("{WS}/SaveSnapshot"), &body).await,
            "invalid_argument",
            "n {n}, label {label:?}"
        );
    }

    // The saved objects are read through the workspace, and only through it.
    let blob = git_env(
        dir.path(),
        &[],
        &["rev-parse", &format!("{tree}:notes/new.txt")],
        b"",
    );
    let has = |workspace: &str| json!({"namespace": "acme", "name": "web", "workspace": workspace, "oids": [blob]});
    let present = op.ok(&format!("{OBJECTS}/HasObjects"), &has(&ws)).await;
    assert_eq!(
        STANDARD
            .decode(present["present"].as_str().expect("bits"))
            .expect("b64"),
        vec![1]
    );
    let absent = op.ok(&format!("{OBJECTS}/HasObjects"), &has("")).await;
    assert_eq!(
        STANDARD
            .decode(absent["present"].as_str().unwrap_or(""))
            .unwrap_or_default(),
        vec![0]
    );
    let (messages, end) = op
        .stream(
            &format!("{OBJECTS}/GetObjects"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws, "oids": [blob]}),
        )
        .await;
    assert!(end.is_null(), "{end}");
    assert_eq!(
        STANDARD
            .decode(messages[0]["data"].as_str().expect("data"))
            .expect("b64"),
        b"saved\n"
    );
    running.shutdown().await;
}

#[tokio::test]
async fn save_with_missing_tree_is_object_missing() {
    let Some(dir) = history() else {
        return;
    };
    let running = Running::start().await;
    load(&running, &dir).await;
    let op = Caller::of(&running, None);
    let ws = create_ws(&op, json!({"namespace": "acme", "name": "web"})).await["workspace"]
        .as_str()
        .expect("id")
        .to_owned();
    let head = rev(&dir, "main");
    let tree = tree_with(&dir, &[("a/b/c.txt", "deep\n")], &[]);
    let rpc = format!("{WS}/SaveSnapshot");

    // A pack with the commit only: the worktree tree is missing.
    let (commit, _) = save_objects(&dir, &ws, 1, &tree, &head);
    let only_commit = {
        let name = git_env(
            dir.path(),
            &[],
            &["pack-objects", "-q", "c-out"],
            format!("{commit}\n").as_bytes(),
        );
        let pack = std::fs::read(dir.path().join(format!("c-out-{name}.pack"))).expect("pack");
        let idx = std::fs::read(dir.path().join(format!("c-out-{name}.idx"))).expect("idx");
        LpkWriter::from_parts(pack.into(), idx.into())
            .expect("bundle")
            .2
    };
    let pack = op.put_pack(&ws, &only_commit).await.expect("stored");
    let (status, error) = op
        .call(
            &rpc,
            &save_body(
                &ws,
                1,
                &tree,
                &head,
                &commit,
                std::slice::from_ref(&pack),
                "",
            ),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["code"], "failed_precondition");
    assert_eq!(reason(&error), "object_missing");

    // The trees and the commit, without the new blob: not connected.
    let blob = git_env(
        dir.path(),
        &[],
        &["rev-parse", &format!("{tree}:a/b/c.txt")],
        b"",
    );
    let trees: Vec<String> = [tree.clone(), format!("{tree}:a"), format!("{tree}:a/b")]
        .iter()
        .map(|r| git_env(dir.path(), &[], &["rev-parse", r], b""))
        .chain([commit.clone()])
        .collect();
    let name = git_env(
        dir.path(),
        &[],
        &["pack-objects", "-q", "t-out"],
        (trees.join("\n") + "\n").as_bytes(),
    );
    let pack_bytes = std::fs::read(dir.path().join(format!("t-out-{name}.pack"))).expect("pack");
    let idx = std::fs::read(dir.path().join(format!("t-out-{name}.idx"))).expect("idx");
    let thin = LpkWriter::from_parts(pack_bytes.into(), idx.into())
        .expect("bundle")
        .2;
    let thin = op.put_pack(&ws, &thin).await.expect("stored");
    let (_, error) = op
        .call(&rpc, &save_body(&ws, 1, &tree, &head, &commit, &[thin], ""))
        .await;
    assert_eq!(reason(&error), "object_missing", "{error}");
    assert!(
        error["message"].as_str().unwrap_or("").contains(&blob),
        "{error}"
    );

    // A pack the workspace does not hold.
    let mut unknown = pack.clone();
    unknown["checksum"] = json!("00000000000000000000000000000000deadbeef");
    let (_, error) = op
        .call(
            &rpc,
            &save_body(&ws, 1, &tree, &head, &commit, &[unknown], ""),
        )
        .await;
    assert_eq!(reason(&error), "object_missing", "{error}");

    // With every object uploaded the same save goes through.
    let (status, saved) = save(&op, &dir, &ws, 1, &tree, "").await;
    assert_eq!(status, reqwest::StatusCode::OK, "{saved}");
    running.shutdown().await;
}

#[tokio::test]
async fn watch_ws_snapshot_then_saves_then_heartbeat() {
    let Some(dir) = history() else {
        return;
    };
    let running = Running::start_with(|config| {
        config.git.watch_heartbeat = Duration::from_millis(300);
    })
    .await;
    load(&running, &dir).await;
    let op = Caller::of(&running, None);
    let ws = create_ws(&op, json!({"namespace": "acme", "name": "web"})).await["workspace"]
        .as_str()
        .expect("id")
        .to_owned();
    let tree = tree_with(&dir, &[("watched.txt", "1\n")], &[]);
    let body = json!({"namespace": "acme", "name": "web", "workspace": ws});
    let watching = watch_until(&running, &body, Duration::from_secs(20), |messages| {
        messages
            .iter()
            .skip_while(|m| m.get("snapshot").is_none_or(|s| n_of(s) != 1))
            .any(|m| m.get("heartbeat").is_some())
    });
    let saving = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (status, saved) = save(&op, &dir, &ws, 1, &tree, &mount(1)).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{saved}");
    };
    let (messages, ()) = tokio::join!(watching, saving);
    assert_eq!(n_of(&messages[0]["snapshot"]), 0, "{messages:?}");
    let saved = messages
        .iter()
        .position(|m| m.get("snapshot").is_some_and(|s| n_of(s) == 1))
        .expect("the save");
    assert_eq!(messages[saved]["snapshot"]["worktreeTree"], tree.as_str());
    assert!(
        messages[saved..]
            .iter()
            .any(|m| m.get("heartbeat").is_some())
    );

    // Resuming after snapshot 1 sends no snapshot first; a delete ends the stream.
    let resumed = watch_until(
        &running,
        &json!({"namespace": "acme", "name": "web", "workspace": ws, "afterN": 1}),
        Duration::from_secs(10),
        |messages| !messages.is_empty(),
    )
    .await;
    assert!(resumed[0].get("heartbeat").is_some(), "{:?}", resumed[0]);
    let deleting = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        op.ok(&format!("{WS}/DeleteWorkspace"), &body).await;
    };
    let (ended, ()) = tokio::join!(read_to_end(&running, &body), deleting);
    let last = ended.last().expect("messages");
    assert_eq!(last["snapshot"]["label"], "deleted", "{ended:?}");
    running.shutdown().await;
}

/// A `WatchWorkspace` stream's messages until `done` holds for them or `limit` passes.
async fn watch_until(
    running: &Running,
    body: &Value,
    limit: Duration,
    done: impl Fn(&[Value]) -> bool,
) -> Vec<Value> {
    let mut response = watch(running, body).await;
    let mut buffer = Vec::new();
    let mut messages = Vec::new();
    let deadline = tokio::time::Instant::now() + limit;
    while !done(&messages) {
        let chunk = tokio::time::timeout_at(deadline, response.chunk())
            .await
            .expect("the stream answers in time")
            .expect("a chunk");
        let Some(chunk) = chunk else { break };
        buffer.extend_from_slice(&chunk);
        let parsed = frames(&buffer);
        let used: usize = parsed.iter().map(|(_, data)| data.len() + 5).sum();
        for (flags, data) in &parsed {
            let value: Value = serde_json::from_slice(data).unwrap_or(Value::Null);
            assert_eq!(flags & 0x02, 0, "the stream ended: {value}");
            messages.push(value);
        }
        buffer.drain(..used);
    }
    messages
}

/// A `WatchWorkspace` stream read until it ends (within 20 s): its messages.
async fn read_to_end(running: &Running, body: &Value) -> Vec<Value> {
    let response = watch(running, body).await;
    let bytes = tokio::time::timeout(Duration::from_secs(20), response.bytes())
        .await
        .expect("the stream ends in time")
        .expect("a body");
    frames(&bytes)
        .into_iter()
        .filter(|(flags, _)| flags & 0x02 == 0)
        .map(|(_, data)| serde_json::from_slice(data).unwrap_or(Value::Null))
        .collect()
}

async fn watch(running: &Running, body: &Value) -> reqwest::Response {
    let payload = body.to_string().into_bytes();
    let mut framed = vec![0u8];
    framed.extend_from_slice(&u32::try_from(payload.len()).expect("small").to_be_bytes());
    framed.extend_from_slice(&payload);
    reqwest::Client::new()
        .post(format!("{}{WS}/WatchWorkspace", running.base))
        .header("content-type", "application/connect+json")
        .body(framed)
        .send()
        .await
        .expect("watch")
}

#[tokio::test]
async fn restore_salvage_replays_as_edits() {
    let Some(dir) = history() else {
        return;
    };
    let running = Running::start().await;
    load(&running, &dir).await;
    let op = Caller::of(&running, None);
    let ws = create_ws(&op, json!({"namespace": "acme", "name": "web"})).await["workspace"]
        .as_str()
        .expect("id")
        .to_owned();
    // Writer 1 saves snapshot 1: README.md changed.
    let ours = tree_with(&dir, &[("README.md", "ours\n")], &[]);
    let (status, saved) = save(&op, &dir, &ws, 1, &ours, &mount(1)).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{saved}");
    // Writer 2, also on snapshot 0, changed README.md, src/lib.rs and removed src/deep/x.txt:
    // fenced, its state salvaged.
    let theirs = tree_with(
        &dir,
        &[("README.md", "theirs\n"), ("src/lib.rs", "// theirs\n")],
        &["src/deep/x.txt"],
    );
    let (status, error) = save(&op, &dir, &ws, 1, &theirs, &mount(2)).await;
    assert_eq!(status, reqwest::StatusCode::CONFLICT, "{error}");
    let salvage = op
        .ok(
            &format!("{WS}/ListSalvage"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws}),
        )
        .await["salvage"][0]
        .clone();
    let restore = json!({"namespace": "acme", "name": "web", "workspace": ws,
                         "mount": salvage["mount"], "salvage": salvage["salvage"],
                         "writer": mount(1)});

    let restored = op.ok(&format!("{WS}/RestoreSalvage"), &restore).await;
    let snap = &restored["snapshot"];
    assert_eq!(n_of(snap), 2);
    // README.md keeps writer 1's text and is a conflict; the rest is writer 2's.
    let want = tree_with(
        &dir,
        &[("README.md", "ours\n"), ("src/lib.rs", "// theirs\n")],
        &["src/deep/x.txt"],
    );
    assert_eq!(snap["worktreeTree"], want.as_str());
    assert_eq!(restored["conflicts"], json!(["README.md"]));
    assert_eq!(snap["conflicts"][0]["path"], "README.md");
    let blob = |tree: &str| {
        git_env(
            dir.path(),
            &[],
            &["rev-parse", &format!("{tree}:README.md")],
            b"",
        )
    };
    assert_eq!(snap["conflicts"][0]["ours"], blob(&ours).as_str());
    assert_eq!(snap["conflicts"][0]["theirs"], blob(&theirs).as_str());
    assert_eq!(
        snap["conflicts"][0]["base"],
        blob(&rev(&dir, "main^{tree}")).as_str()
    );
    // The restored snapshot's objects are readable through the workspace.
    let trees = op
        .ok(
            &format!("{OBJECTS}/GetTrees"),
            &json!({"namespace": "acme", "name": "web", "workspace": ws,
                    "root": snap["snapshotCommit"], "depth": 1}),
        )
        .await;
    assert_eq!(trees["trees"][0]["oid"], want.as_str());
    // Restoring again changes nothing.
    let again = op.ok(&format!("{WS}/RestoreSalvage"), &restore).await;
    assert_eq!(n_of(&again["snapshot"]), 2);
    running.shutdown().await;
}

#[tokio::test]
async fn fork_shares_packs_and_stops_at_depth() {
    let Some(dir) = history() else {
        return;
    };
    let running = Running::start().await;
    load(&running, &dir).await;
    let op = Caller::of(&running, None);
    let source = create_ws(&op, json!({"namespace": "acme", "name": "web"})).await["workspace"]
        .as_str()
        .expect("id")
        .to_owned();
    let tree = tree_with(&dir, &[("forked.txt", "shared\n")], &[]);
    let (status, saved) = save(&op, &dir, &source, 1, &tree, "").await;
    assert_eq!(status, reqwest::StatusCode::OK, "{saved}");
    let blob = git_env(
        dir.path(),
        &[],
        &["rev-parse", &format!("{tree}:forked.txt")],
        b"",
    );
    // Eight forks deep, each reading the source's pack through the chain.
    let mut chain = vec![source.clone()];
    for depth in 1..=8 {
        let fork = op
            .ok(
                &format!("{WS}/ForkWorkspace"),
                &json!({"namespace": "acme", "name": "web", "workspace": chain.last(),
                        "idempotencyKey": format!("f{depth}")}),
            )
            .await["snapshot"]
            .clone();
        assert_eq!(n_of(&fork), 0);
        assert_eq!(fork["worktreeTree"], tree.as_str());
        assert_eq!(
            fork["fork"]["workspace"],
            chain.last().expect("a source").as_str()
        );
        assert_eq!(fork["packs"], saved["snapshot"]["packs"]);
        chain.push(fork["workspace"].as_str().expect("id").to_owned());
    }
    let deepest = chain.last().expect("the chain").clone();
    let present = op
        .ok(
            &format!("{OBJECTS}/HasObjects"),
            &json!({"namespace": "acme", "name": "web", "workspace": deepest, "oids": [blob]}),
        )
        .await;
    assert_eq!(
        STANDARD
            .decode(present["present"].as_str().expect("bits"))
            .expect("b64"),
        vec![1]
    );
    // A save on the deepest fork names the inherited pack.
    let (status, saved_fork) = save(&op, &dir, &deepest, 1, &tree, "").await;
    assert_eq!(status, reqwest::StatusCode::OK, "{saved_fork}");
    assert_eq!(
        op.refused(
            &format!("{WS}/ForkWorkspace"),
            &json!({"namespace": "acme", "name": "web", "workspace": deepest}),
        )
        .await,
        "fork_too_deep"
    );
    running.shutdown().await;
}

/// An engine with authentication on: the owner (an org owner, who set it up), and two members,
/// mel and bob, with an empty repository `acme/web`.
struct Team {
    running: Running,
    owner: Client,
    mel: Caller,
    bob: Caller,
}

impl Team {
    async fn start(owner_reads: bool) -> Self {
        let running = Running::start_with(|config| {
            config.auth = AuthConfig {
                setup_token: Some(SETUP_TOKEN.to_owned()),
                extra_authenticators: Authenticators(vec![Arc::new(TestAuthenticator)]),
                ..AuthConfig::off()
            };
            config.git.workspace_reads_owner_only = owner_reads;
        })
        .await;
        let mut owner = Client::new(&running.base);
        owner.setup(SETUP_TOKEN).await;
        for who in ["mel", "bob"] {
            owner
                .call(
                    "admin.v1.OrgService",
                    "CreateInvitation",
                    json!({"email": format!("{who}@example.test")}),
                )
                .await
                .ok();
        }
        owner
            .call(
                "repos.v1.RepoService",
                "CreateRepo",
                json!({"namespace": "acme", "name": "web", "idempotencyKey": "r1"}),
            )
            .await
            .ok();
        let mel = Caller::of(&running, Some(&TestAuthenticator::token("mel")));
        let bob = Caller::of(&running, Some(&TestAuthenticator::token("bob")));
        Self {
            running,
            owner,
            mel,
            bob,
        }
    }
}

#[tokio::test]
async fn other_owner_cannot_write_or_take_over() {
    let team = Team::start(false).await;
    let ws = create_ws(&team.mel, json!({"namespace": "acme", "name": "web"})).await;
    assert_eq!(ws["owner"], "test|mel");
    // An unborn workspace: the repository has no commit yet.
    assert_eq!(ws["head"].as_str().unwrap_or(""), "");
    let id = ws["workspace"].as_str().expect("id").to_owned();
    let empty = ws["worktreeTree"].as_str().expect("tree").to_owned();
    let body = json!({"namespace": "acme", "name": "web", "workspace": id});
    let mut takeover = save_body(&id, 1, &empty, "", &empty, &[], &mount(9));
    takeover["snapshot"]["takeover"] = json!({"fromMount": "", "fromN": 0, "reason": "take"});
    let restore = json!({"namespace": "acme", "name": "web", "workspace": id,
                         "mount": mount(1), "salvage": "01j9z3m5q8x000000000000001"});
    for (rpc, body) in [
        ("SaveSnapshot", &takeover),
        ("DeleteWorkspace", &body),
        ("RestoreSalvage", &restore),
    ] {
        assert_eq!(
            team.bob.refused(&format!("{WS}/{rpc}"), body).await,
            "permission_denied",
            "{rpc}"
        );
    }
    let refused = team
        .bob
        .put_pack(&id, b"not a pack")
        .await
        .expect_err("refused");
    assert_eq!(reason(&refused), "permission_denied");
    // Bob may fork it (reading it), and owns the fork.
    let fork = team.bob.ok(&format!("{WS}/ForkWorkspace"), &body).await;
    assert_eq!(fork["snapshot"]["owner"], "test|bob");
    // An org admin may delete it.
    let deleted = team
        .owner
        .call("repos.v1.WorkspaceService", "DeleteWorkspace", body.clone())
        .await;
    assert_eq!(deleted.ok()["snapshot"]["label"], "deleted");
    team.running.shutdown().await;
}

#[tokio::test]
async fn repo_reader_can_read_ws() {
    for owner_reads in [false, true] {
        let team = Team::start(owner_reads).await;
        let ws = create_ws(
            &team.mel,
            json!({"namespace": "acme", "name": "web", "workspaceName": "mine"}),
        )
        .await;
        let id = ws["workspace"].as_str().expect("id").to_owned();
        let body = json!({"namespace": "acme", "name": "web", "workspace": id});
        let has = json!({"namespace": "acme", "name": "web", "workspace": id,
                         "oids": [ws["worktreeTree"]]});
        let mels = json!({"namespace": "acme", "name": "web", "owner": "test|mel"});
        let reads = [
            (format!("{WS}/GetWorkspace"), body.clone()),
            (format!("{WS}/ListSnapshots"), body.clone()),
            (format!("{WS}/ListSalvage"), body.clone()),
            (format!("{WS}/ListWorkspaces"), mels.clone()),
            (format!("{OBJECTS}/HasObjects"), has.clone()),
        ];
        for (rpc, body) in &reads {
            team.mel.ok(rpc, body).await;
            if owner_reads {
                assert_eq!(
                    team.bob.refused(rpc, body).await,
                    "permission_denied",
                    "{rpc}"
                );
            } else {
                team.bob.ok(rpc, body).await;
            }
        }
        // Listing every workspace is an admin's when reads are owners'.
        let all = json!({"namespace": "acme", "name": "web"});
        if owner_reads {
            assert_eq!(
                team.bob
                    .refused(&format!("{WS}/ListWorkspaces"), &all)
                    .await,
                "permission_denied"
            );
        } else {
            team.bob.ok(&format!("{WS}/ListWorkspaces"), &all).await;
        }
        // The org admin reads either way.
        let answer = team
            .owner
            .call("repos.v1.WorkspaceService", "GetWorkspace", body.clone())
            .await;
        assert_eq!(answer.ok()["snapshot"]["workspace"], id.as_str());
        team.running.shutdown().await;
    }
}

/// Design 76 §9 (plan IA1, ruling IA2.R4): `RepoService` asks model v1 on the repository's
/// namespace as well as the org role, so an org member reaches a project's namespaces only
/// when granted the project, and the org's own namespaces as before.
#[tokio::test]
async fn a_member_reaches_repos_only_in_granted_projects() {
    let team = Team::start(false).await;
    const ORG: &str = "admin.v1.OrgService";
    const PROJECTS: &str = "admin.v1.ProjectService";
    const REPOS: &str = "/loams.repos.v1.RepoService";
    // Both accept their invitations.
    team.mel
        .ok(&format!("/loams.{ORG}/GetOrg"), &json!({}))
        .await;
    team.bob
        .ok(&format!("/loams.{ORG}/GetOrg"), &json!({}))
        .await;
    team.owner
        .call(PROJECTS, "CreateProject", json!({"name": "Shop"}))
        .await
        .ok();
    team.owner
        .call(
            PROJECTS,
            "CreateEnvironment",
            json!({"project": "shop", "name": "Dev"}),
        )
        .await
        .ok();
    team.owner
        .call(
            "repos.v1.RepoService",
            "CreateRepo",
            json!({"namespace": "shop-dev", "name": "api", "idempotencyKey": "r2"}),
        )
        .await
        .ok();
    let members = team.owner.call(ORG, "ListMembers", json!({})).await;
    let mel = members.ok()["members"]
        .as_array()
        .expect("members")
        .iter()
        .find(|m| m["user"]["email"] == "mel@example.test")
        .map(|m| m["user"]["id"].as_str().expect("id").to_owned())
        .expect("mel");
    team.owner
        .call(
            PROJECTS,
            "GrantAccess",
            json!({"project": "shop",
                   "subject": {"kind": "SUBJECT_KIND_USER", "id": mel},
                   "role": "PROJECT_ROLE_DEVELOPER"}),
        )
        .await
        .ok();
    let api = json!({"namespace": "shop-dev", "name": "api"});
    team.mel.ok(&format!("{REPOS}/GetRepo"), &api).await;
    assert_eq!(
        team.bob.refused(&format!("{REPOS}/GetRepo"), &api).await,
        "role_required",
        "bob has no grant on shop"
    );
    // The org's own namespace: every member, as before.
    let web = json!({"namespace": "acme", "name": "web"});
    team.bob.ok(&format!("{REPOS}/GetRepo"), &web).await;
    // A developer writes but does not administer the project's repositories.
    assert_eq!(
        team.mel
            .refused(
                &format!("{REPOS}/DeleteRepo"),
                &json!({"namespace": "shop-dev", "name": "api", "idempotencyKey": "d1"})
            )
            .await,
        "role_required"
    );
    team.running.shutdown().await;
}
