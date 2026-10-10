//! LV1 plan Task 5 (design §45 §3.1, Review Focus 3): an isolated worker
//! cannot open files, sockets or processes. A test-only probe asks the
//! worker to make the system call after its sandbox is applied; the
//! seccomp filter kills it with `SIGSYS`, and the probe, like a call,
//! fails with `FUNCTION_ERROR`, reason `live_worker_crashed`.

#![cfg(target_os = "linux")]

mod common;

use common::*;
use loams_live::{Isolation, LiveError, pb};
use loams_live_js::{JsConfig, Probe, WorkerHandle};

const BUNDLE: &str = r#"
import { query } from "loams:server";
export const s = { ok: query(async () => "fine") };
"#;

async fn worker() -> WorkerHandle {
    workers()
        .spawn(
            "sandbox-test",
            BUNDLE.as_bytes(),
            JsConfig {
                isolation: Isolation::Isolated,
                contexts: 1,
                ..JsConfig::default()
            },
        )
        .await
        .expect("the worker loads the bundle")
}

/// The worker survives a probe that makes no forbidden call, so a killed
/// probe is the sandbox's doing.
async fn assert_killed_by_seccomp(probe: Probe) {
    let handle = worker().await;
    handle
        .probe(Probe::Ping)
        .await
        .expect("a ping survives the sandbox");
    match handle.probe(probe).await {
        Err(e @ LiveError::WorkerCrashed(_)) => {
            assert_eq!(e.code(), pb::ErrorCode::ERROR_CODE_FUNCTION_ERROR);
            assert_eq!(e.reason(), Some("live_worker_crashed"));
            assert!(e.to_string().contains("SIGSYS"), "{probe:?}: {e}");
        }
        other => panic!("{probe:?}: the worker dies with SIGSYS, not {other:?}"),
    }
    assert_eq!(handle.crashes(), 1);
    // The deployment keeps serving, on a fresh worker.
    let store = loams_live::testing::TestStore::embedded(option_env!("CARGO_TARGET_TMPDIR")).await;
    let r = runner(&store).await;
    let ok = handle.function("s:ok").expect("s:ok");
    assert_eq!(query(&r, &ok, unit()).await.expect("ok").result, s("fine"));
}

#[tokio::test]
async fn worker_cannot_open_files() {
    assert_killed_by_seccomp(Probe::OpenFile).await;
}

#[tokio::test]
async fn worker_cannot_open_sockets() {
    assert_killed_by_seccomp(Probe::OpenSocket).await;
}

#[tokio::test]
async fn worker_cannot_exec() {
    assert_killed_by_seccomp(Probe::Exec).await;
}
