//! Loams Git in the `loams` binary (GT1, design §36): `loams.repos.v1` on the main port, and what
//! the default build leaves out.
//!
//! Built twice: without the `git` feature every `RepoService` RPC answers
//! `feature_not_in_variant`; with it (`cargo test -p loams --features git --test git`) the
//! package is served.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::process::Command;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use buffa::Message as _;
use loams::{Server, ServerConfig};
use loams_proto::loams::errors::v1::ErrorInfo;
use serde_json::{Value, json};
use tempfile::TempDir;

#[cfg(feature = "git")]
#[path = "../console_support/mod.rs"]
mod console_support;
#[cfg(feature = "git")]
mod http;
#[cfg(feature = "git")]
mod objects;
#[cfg(feature = "git")]
mod repos;
#[cfg(feature = "git")]
mod workspaces;
#[cfg(feature = "git")]
mod ws_edits;
#[cfg(feature = "git")]
mod ws_push;

/// Every RPC of `RepoService`, with whether it streams.
const RPCS: &[(&str, &str, bool)] = &[
    ("RepoService", "CreateRepo", false),
    ("RepoService", "GetRepo", false),
    ("RepoService", "ListRepos", false),
    ("RepoService", "ForkRepo", false),
    ("RepoService", "DeleteRepo", false),
    ("RepoService", "UpdateRepoConfig", false),
    ("RepoService", "ListRefs", false),
    ("RepoService", "CompactRepo", false),
    ("RepoService", "WatchRepo", true),
    ("ObjectService", "GetObjects", true),
    ("ObjectService", "GetTrees", false),
    ("ObjectService", "HasObjects", false),
    ("ObjectService", "Log", true),
    ("ObjectService", "DiffStat", false),
    ("ObjectService", "PutWorkspacePack", true),
    ("WorkspaceService", "CreateWorkspace", false),
    ("WorkspaceService", "GetWorkspace", false),
    ("WorkspaceService", "ListWorkspaces", false),
    ("WorkspaceService", "ForkWorkspace", false),
    ("WorkspaceService", "DeleteWorkspace", false),
    ("WorkspaceService", "ListSnapshots", false),
    ("WorkspaceService", "GetSnapshot", false),
    ("WorkspaceService", "SaveSnapshot", false),
    ("WorkspaceService", "WatchWorkspace", true),
    ("WorkspaceService", "ListSalvage", false),
    ("WorkspaceService", "RestoreSalvage", false),
    ("WorkspaceService", "ReadFiles", true),
    ("WorkspaceService", "ApplyEdits", false),
    ("WorkspaceService", "Diff", true),
    ("WorkspaceService", "Rebase", false),
    ("WorkspaceService", "CommitWorkspace", false),
    ("WorkspaceService", "PushWorkspace", false),
];

/// A `loams` server on loopback, with its data directory.
pub(crate) struct Running {
    pub(crate) server: Server,
    pub(crate) base: String,
    _dir: TempDir,
}

impl Running {
    pub(crate) async fn start_with(edit: impl FnOnce(&mut ServerConfig)) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let mut config = ServerConfig::new(dir.path());
        config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
        config.log.flush_interval = Duration::from_millis(20);
        config.worker_poll_interval = Duration::from_millis(50);
        edit(&mut config);
        let server = Server::start(config).await.expect("start");
        let base = format!("http://{}", server.local_addr());
        Self {
            server,
            base,
            _dir: dir,
        }
    }

    pub(crate) async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    /// A Connect unary call with a JSON body.
    pub(crate) async fn connect(&self, rpc: &str, body: &Value) -> (reqwest::StatusCode, Value) {
        let response = reqwest::Client::new()
            .post(format!("{}{rpc}", self.base))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .expect("connect call");
        let status = response.status();
        let text = response.text().await.expect("a body");
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    /// A Connect server-streaming call with a JSON body, read to its end: every message, and
    /// the end-of-stream message.
    pub(crate) async fn connect_stream(
        &self,
        rpc: &str,
        body: &Value,
    ) -> (reqwest::StatusCode, Vec<(u8, Value)>) {
        let payload = body.to_string().into_bytes();
        let mut framed = vec![0u8];
        framed.extend_from_slice(&u32::try_from(payload.len()).expect("small").to_be_bytes());
        framed.extend_from_slice(&payload);
        let response = reqwest::Client::new()
            .post(format!("{}{rpc}", self.base))
            .header("content-type", "application/connect+json")
            .body(framed)
            .send()
            .await
            .expect("connect stream call");
        let status = response.status();
        let bytes = response.bytes().await.expect("a body");
        let messages = frames(&bytes)
            .into_iter()
            .map(|(flags, data)| (flags, serde_json::from_slice(data).unwrap_or(Value::Null)))
            .collect();
        (status, messages)
    }

    /// The server's bucket (`<data_dir>/bucket`), as a second writer would open it.
    #[cfg(feature = "git")]
    pub(crate) fn bucket(&self) -> loams_store::Store {
        let dir = self
            ._dir
            .path()
            .join("bucket")
            .canonicalize()
            .expect("bucket dir");
        let url = url::Url::from_directory_path(&dir).expect("an absolute path");
        loams_store::Store::from_url(url.as_str(), Vec::<(String, String)>::new()).expect("store")
    }

    pub(crate) async fn shutdown(self) {
        self.server.shutdown().await.expect("shutdown");
    }
}

/// The length-prefixed frames of a body.
pub(crate) fn frames(body: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 5 <= body.len() {
        let len =
            u32::from_be_bytes([body[at + 1], body[at + 2], body[at + 3], body[at + 4]]) as usize;
        let end = at + 5 + len;
        assert!(end <= body.len(), "a frame runs past the body");
        out.push((body[at], &body[at + 5..end]));
        at = end;
    }
    out
}

/// The `ErrorInfo.reason` of a Connect JSON error body.
pub(crate) fn reason(error: &Value) -> String {
    let detail = &error["details"][0];
    assert_eq!(detail["type"], "loams.errors.v1.ErrorInfo", "{error}");
    let bytes = STANDARD_NO_PAD
        .decode(detail["value"].as_str().expect("an encoded detail"))
        .expect("base64");
    ErrorInfo::decode_from_slice(&bytes)
        .expect("ErrorInfo")
        .reason
}

/// The workspace root.
fn workspace() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `cargo tree -p loams -e normal` with `features`, as text.
fn cargo_tree(features: &[&str]) -> String {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .current_dir(workspace())
        .args([
            "tree",
            "-p",
            "loams",
            "-e",
            "normal",
            "--locked",
            "--offline",
        ])
        .args(["--prefix", "none", "--no-default-features", "--features"])
        .arg(features.join(","))
        .output()
        .expect("cargo tree runs");
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

/// The packages of a `cargo tree --prefix none` listing.
fn packages(tree: &str) -> BTreeSet<String> {
    tree.lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// The default build links no Loams Git code (plan GT1 Global Constraints, Review Focus 14):
/// none of `loams-git` or the gitoxide crates. The generated `loams.repos.v1` types are in
/// every build (ruling T0-10), so the check is on crates, not types.
#[test]
fn default_features_exclude_git() {
    let default = packages(&cargo_tree(&["default"]));
    assert!(default.contains("loams"), "the listing parsed: {default:?}");
    for name in [
        "loams-git",
        "loams-vfs",
        "loams-env",
        "loams-vault",
        "loams-buildcache",
        "loams-registry",
        "gix-pack",
        "gix-object",
        "gix-hash",
    ] {
        assert!(!default.contains(name), "{name} is in the default build");
    }
    // The control: the feature does bring them in, so the check above reads a real listing.
    let with = packages(&cargo_tree(&["default", "git"]));
    assert!(with.contains("loams-git"), "{with:?}");
    assert!(with.contains("gix-pack"), "{with:?}");
}

/// `GetInstance` lists `loams.repos.v1` as available exactly when the build has the `git`
/// feature, and every RPC of a build without it answers `feature_not_in_variant`.
#[tokio::test]
async fn catalogue_lists_repos_only_with_feature() {
    let running = Running::start().await;
    let (status, info) = running
        .connect("/loams.instance.v1.InstanceService/GetInstance", &json!({}))
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{info}");
    let row = info["services"]
        .as_array()
        .expect("services")
        .iter()
        .find(|s| s["package"] == "loams.repos.v1")
        .cloned()
        .unwrap_or_else(|| panic!("no loams.repos.v1 row: {info}"));
    assert_eq!(
        row["services"],
        json!([
            "loams.repos.v1.RepoService",
            "loams.repos.v1.ObjectService",
            "loams.repos.v1.WorkspaceService"
        ])
    );
    assert_eq!(row["unstable"], json!(true));
    let served = cfg!(feature = "git");
    assert_eq!(row["available"].as_bool().unwrap_or(false), served, "{row}");
    let versions: Vec<&str> = info["apiVersions"]
        .as_array()
        .expect("apiVersions")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(versions.contains(&"loams.repos.v1"), served, "{versions:?}");
    // A `git` build serves them (`repos.rs`); every RPC of a build without it is refused.
    let rpcs = if served { &[][..] } else { RPCS };
    for (service, rpc, streams) in rpcs {
        let path = format!("/loams.repos.v1.{service}/{rpc}");
        let body = json!({"namespace": "default", "name": "app"});
        let error = if *streams {
            let (_, messages) = running.connect_stream(&path, &body).await;
            let (_, end) = messages
                .into_iter()
                .find(|(flags, _)| flags & 0x02 != 0)
                .expect("an end-of-stream message");
            end["error"].clone()
        } else {
            let (status, error) = running.connect(&path, &body).await;
            assert_eq!(
                status,
                reqwest::StatusCode::NOT_IMPLEMENTED,
                "{rpc}: {error}"
            );
            error
        };
        assert_eq!(error["code"], "unimplemented", "{rpc}: {error}");
        assert_eq!(reason(&error), "feature_not_in_variant", "{rpc}");
    }
    running.shutdown().await;
}
