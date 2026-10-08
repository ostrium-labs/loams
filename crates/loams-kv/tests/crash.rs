//! Crash atomicity of the embedded store (LV1 plan Task 21): a child process
//! commits multi-key transactions and is `SIGKILL`ed at a random point;
//! after reopening, every transaction is either fully visible or absent,
//! and every one the child saw commit is visible.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use loams_kv::testing::TempDir;
use loams_kv::{EmbeddedConfig, Store, StoreConfig, TxnOptions};

/// Set in the child: the store file it commits to.
const CHILD_ENV: &str = "LOAMS_KV_CRASH_CHILD";
const TXNS: u32 = 1_000;
const KEYS_PER_TXN: u32 = 5;

fn key(txn: u32, k: u32) -> Vec<u8> {
    format!("t{txn:04}/{k}").into_bytes()
}

async fn open(path: &Path) -> Store {
    Store::open(StoreConfig::Embedded(EmbeddedConfig::new(
        path.to_path_buf(),
        "crash",
    )))
    .await
    .expect("an embedded store")
}

/// The child: commits [`TXNS`] transactions of [`KEYS_PER_TXN`] keys each,
/// printing `committed <i>` after each. Does nothing outside the crash test.
#[tokio::test]
async fn crash_child() {
    let Some(path) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let store = open(Path::new(&path)).await;
    for i in 0..TXNS {
        store
            .run(TxnOptions::new("kv.crash"), move |txn| {
                Box::pin(async move {
                    for k in 0..KEYS_PER_TXN {
                        txn.put(&key(i, k), i.to_be_bytes().to_vec()).await?;
                    }
                    Ok(())
                })
            })
            .await
            .expect("committed");
        println!("committed {i}");
    }
}

#[tokio::test]
async fn crash_mid_commit_is_atomic() {
    if std::env::var_os(CHILD_ENV).is_some() {
        return;
    }
    let dir = TempDir::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).expect("a directory");
    let path = dir.path().join("store.redb");
    let mut child = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", "crash_child", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, &path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the child starts");
    let stdout = child.stdout.take().expect("piped");
    let (acks, acked) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(i) = line.strip_prefix("committed ")
                && let Ok(i) = i.trim().parse::<u32>()
                && acks.send(i).is_err()
            {
                return;
            }
        }
    });
    let target = rand::random_range(20..TXNS - 100);
    let mut last = None;
    while last.is_none_or(|l| l < target) {
        match acked.recv_timeout(Duration::from_secs(60)) {
            Ok(i) => last = Some(i),
            Err(e) => panic!("the child stopped acknowledging ({e}) after {last:?}"),
        }
    }
    child.kill().expect("SIGKILL");
    child.wait().expect("reaped");
    // Acknowledgements printed before the kill.
    while let Ok(i) = acked.try_recv() {
        last = Some(i);
    }
    let last = last.expect("acknowledged");

    let store = open(&path).await;
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    let mut visible = 0;
    for i in 0..TXNS {
        let keys: Vec<Vec<u8>> = (0..KEYS_PER_TXN).map(|k| key(i, k)).collect();
        let found = snap.batch_get(keys).await.expect("batch get");
        assert!(
            found.is_empty() || found.len() == KEYS_PER_TXN as usize,
            "transaction {i} is partly visible: {} of {KEYS_PER_TXN} keys",
            found.len()
        );
        assert!(
            found.iter().all(|(_, v)| v.as_slice() == i.to_be_bytes()),
            "transaction {i} has another's value"
        );
        if i <= last {
            assert_eq!(
                found.len(),
                KEYS_PER_TXN as usize,
                "transaction {i} was acknowledged (up to {last}) but is gone"
            );
        }
        if !found.is_empty() {
            visible += 1;
        }
    }
    assert!(visible > last, "{visible} visible, {last} acknowledged");
    assert!(
        visible < TXNS,
        "the kill landed after the last commit; the test proved nothing"
    );
}
