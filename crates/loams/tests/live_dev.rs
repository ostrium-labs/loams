//! `loams dev` with Loams Live on the embedded store (LV1 plan Task 23): Live
//! runs by default with no PD, its data under `<data_dir>/live/`, and the
//! data survives a restart.
//!
//! Run with `cargo test -p loams --test live_dev` (the `live` feature is in
//! the default set).

#![cfg(feature = "live")]

use std::io::BufRead;
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

/// A running `loams dev`: its HTTP and Live addresses. Killed on drop.
struct Dev {
    child: Child,
    http: SocketAddr,
    live: SocketAddr,
}

impl Drop for Dev {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn addr_after(line: &str, prefix: &str) -> Option<SocketAddr> {
    line.strip_prefix(prefix)?.trim().parse().ok()
}

/// `loams dev` on `data_dir`, every listener on an ephemeral loopback port,
/// with no PD configured.
fn start(data_dir: &Path) -> Dev {
    let mut child = Command::new(env!("CARGO_BIN_EXE_loams"))
        .args([
            "dev",
            "--listen",
            "127.0.0.1:0",
            "--flight-sql-listen",
            "127.0.0.1:0",
            "--es-listen",
            "127.0.0.1:0",
            "--no-qdrant",
            "--live-listen",
            "127.0.0.1:0",
            "--data-dir",
        ])
        .arg(data_dir)
        .env_remove("LOAMS_TEST_PD")
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn loams");
    let stdout = child.stdout.take().expect("stdout");
    let mut lines = std::io::BufReader::new(stdout).lines();
    let (mut http, mut live, mut seen) = (None, None, Vec::new());
    while http.is_none() {
        let line = lines
            .next()
            .unwrap_or_else(|| panic!("loams exited before listening; printed {seen:?}"))
            .expect("read stdout");
        live = live.or_else(|| addr_after(&line, "loams live listening on http://"));
        http = addr_after(&line, "loams listening on http://");
        seen.push(line);
    }
    std::thread::spawn(move || for _ in lines {});
    let live = live.unwrap_or_else(|| panic!("no Live line before the HTTP line: {seen:?}"));
    Dev {
        child,
        http: http.expect("the HTTP address"),
        live,
    }
}

/// Stops `dev` with SIGTERM and waits for it to exit.
fn stop(mut dev: Dev) {
    let sent = Command::new("kill")
        .args(["-TERM", &dev.child.id().to_string()])
        .status()
        .expect("kill");
    assert!(sent.success());
    for _ in 0..200 {
        if dev.child.try_wait().expect("wait").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("loams did not stop within 10 s of SIGTERM");
}

/// A Connect JSON unary call: the status and the response body.
async fn connect(addr: SocketAddr, path: &str, body: &Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .header("content-type", "application/json")
        .json(body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("a response");
    let status = response.status().as_u16();
    let text = response.text().await.expect("a body");
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

fn object(fields: Value) -> Value {
    json!({ "objectValue": { "fields": fields } })
}

/// `_system:insert` of `{ n }` into `notes`: the new document's id and the
/// commit timestamp.
async fn insert(live: SocketAddr, n: i64) -> (Value, Value) {
    let args = object(json!({
        "table": { "stringValue": "notes" },
        "fields": object(json!({ "n": { "int64Value": n.to_string() } })),
    }));
    let (status, body) = connect(
        live,
        "/loams.live.v1.LiveService/Mutate",
        &json!({ "function": "_system:insert", "args": args }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    (body["result"].clone(), body["commitTs"].clone())
}

/// `_system:get` of `id` at timestamp `ts` (a Query without one reads at
/// the latest subscription tick, which may not have reached a commit just
/// made): the document's `n`.
async fn get_n(live: SocketAddr, id: &Value, ts: &Value) -> Value {
    let (status, body) = connect(
        live,
        "/loams.live.v1.LiveService/Query",
        &json!({
            "function": "_system:get",
            "args": object(json!({ "id": id })),
            "ts": ts,
        }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    body["result"]["objectValue"]["fields"]["n"]["int64Value"].clone()
}

/// With no PD anywhere, `loams dev` serves `GetInstance` and Live on the
/// embedded store under `<data_dir>/live/`: an insert then a get round-trip.
#[tokio::test(flavor = "multi_thread")]
async fn dev_starts_live_without_pd() {
    let dir = TempDir::new().expect("a temp dir");
    let dev = start(dir.path());
    let (status, info) = connect(
        dev.http,
        "/loams.instance.v1.InstanceService/GetInstance",
        &json!({}),
    )
    .await;
    assert_eq!(status, 200, "{info}");
    let (id, ts) = insert(dev.live, 7).await;
    assert!(id["stringValue"].is_string(), "an id: {id}");
    assert_eq!(get_n(dev.live, &id, &ts).await, json!("7"));
    assert!(
        dir.path().join("live").join("store.redb").exists(),
        "the store is <data_dir>/live/store.redb"
    );
    drop(dev);
}

/// The embedded store keeps Live's data across a restart of `loams dev`.
#[tokio::test(flavor = "multi_thread")]
async fn data_survives_restart_on_embedded() {
    let dir = TempDir::new().expect("a temp dir");
    let dev = start(dir.path());
    let (id, ts) = insert(dev.live, 41).await;
    stop(dev);
    let dev = start(dir.path());
    assert_eq!(get_n(dev.live, &id, &ts).await, json!("41"));
    let (other, _) = insert(dev.live, 42).await;
    assert_ne!(other, id);
    drop(dev);
}

/// The same across a SIGKILL: a committed insert is durable without a clean
/// shutdown (Task 23 review item 12).
#[tokio::test(flavor = "multi_thread")]
async fn data_survives_sigkill_on_embedded() {
    let dir = TempDir::new().expect("a temp dir");
    let mut dev = start(dir.path());
    let (id, ts) = insert(dev.live, 43).await;
    dev.child.kill().expect("SIGKILL");
    dev.child.wait().expect("reaped");
    drop(dev);
    let dev = start(dir.path());
    assert_eq!(get_n(dev.live, &id, &ts).await, json!("43"));
    drop(dev);
}
