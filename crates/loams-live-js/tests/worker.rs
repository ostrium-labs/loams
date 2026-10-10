//! LV1 plan Task 5 (design §45 §3.1, D681; rows T3-10, T5-*): functions in
//! an isolated worker process. A worker that crashes fails its call with
//! `FUNCTION_ERROR` (reason `live_worker_crashed`) and is respawned; five
//! crashes in a minute mark the deployment degraded; a call stuck in a C
//! built-in that never polls the interrupt handler is killed from outside
//! after `cpu_limit` plus a grace; `tenancy = "multi"` needs `isolated`.

mod common;

use std::time::{Duration, Instant};

use common::*;
use loams_live::testing::TestStore;
use loams_live::{
    Isolation, LiveConfig, LiveError, LiveValue, Tenancy, check_isolation, live_test, pb,
};
use loams_live_js::{JsConfig, WorkerHandle};

const BUNDLE: &str = r#"
import { query, mutation } from "loams:server";

export const w = {
  ok: query(async () => "fine"),
  spin: query(async () => { for (;;) {} }),
  get: query(async (ctx, { id }) => await ctx.db.get(id)),
  gets: query(async (ctx, { id, n }) => {
    for (let i = 0; i < n; i++) await ctx.db.get(id);
    return n;
  }),
  insert: mutation(async (ctx, { body }) => await ctx.db.insert("notes", { body })),
  // C built-ins that loop without polling the interrupt handler (LV1 row
  // T3-7): in process, the first ran 10 s (to the transaction's deadline)
  // and the second 1.3 s past a 200 ms limit. Only a kill stops them.
  stuckStringify: query(async () => { const a = []; a.length = 2 ** 32 - 1; return JSON.stringify(a).length; }),
  stuckRepeat: query(async () => "ab".repeat(2 ** 28).length),
};
"#;

fn isolated(config: JsConfig) -> JsConfig {
    JsConfig {
        isolation: Isolation::Isolated,
        ..config
    }
}

async fn spawn(config: JsConfig) -> WorkerHandle {
    workers()
        .spawn("worker-test", BUNDLE.as_bytes(), isolated(config))
        .await
        .expect("the worker loads the bundle")
}

fn expect_crash(result: Result<impl std::fmt::Debug, LiveError>, what: &str) -> String {
    match result {
        Err(e @ LiveError::WorkerCrashed(_)) => {
            assert_eq!(e.code(), pb::ErrorCode::ERROR_CODE_FUNCTION_ERROR, "{what}");
            assert_eq!(e.reason(), Some("live_worker_crashed"), "{what}");
            e.to_string()
        }
        other => panic!("{what}: live_worker_crashed, not {other:?}"),
    }
}

/// Kills every worker process of `handle` from outside, as the OOM killer
/// or a crash would.
fn kill_workers(handle: &WorkerHandle) {
    for pid in handle.pids() {
        let status = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .expect("kill runs");
        assert!(status.success(), "kill {pid}");
    }
}

/// A worker that dies in the middle of a call fails that call with
/// `live_worker_crashed`, and the next call runs on a fresh worker.
#[cfg(target_os = "linux")]
async fn worker_crash_is_function_error_and_respawns(store: TestStore) {
    let r = runner(&store).await;
    let handle = spawn(JsConfig {
        cpu_limit: Duration::from_secs(20),
        contexts: 1,
        ..JsConfig::default()
    })
    .await;
    let ok = handle.function("w:ok").expect("w:ok");
    assert_eq!(query(&r, &ok, unit()).await.expect("ok").result, s("fine"));
    let before = handle.pids();
    assert_eq!(before.len(), 1, "one worker serves the calls");

    let spin = handle.function("w:spin").expect("w:spin");
    let started = Instant::now();
    let killer = {
        let handle = handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            kill_workers(&handle);
        })
    };
    let message = expect_crash(query(&r, &spin, unit()).await, "a killed call");
    killer.await.expect("the killer ran");
    assert!(message.contains("SIGKILL"), "{message}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the crash is noticed at once"
    );
    assert_eq!(handle.crashes(), 1);
    assert!(!handle.degraded());

    // A mutation is not retried after a crash (its transaction never
    // committed), and the next call runs on a fresh worker.
    assert_eq!(query(&r, &ok, unit()).await.expect("ok").result, s("fine"));
    let after = handle.pids();
    assert_eq!(after.len(), 1);
    assert_ne!(before, after, "a fresh worker process");
    let insert = handle.function("w:insert").expect("w:insert");
    let m = mutate(&r, &insert, obj(&[("body", s("hi"))]))
        .await
        .expect("a mutation commits on the fresh worker");
    let get = handle.function("w:get").expect("w:get");
    let doc = query(&r, &get, obj(&[("id", m.result.clone())]))
        .await
        .expect("get");
    assert_eq!(field(&doc.result, "body"), s("hi"));
}
#[cfg(target_os = "linux")]
live_test!(worker_crash_is_function_error_and_respawns);

/// A worker that dies while idle never ran a call: the next call starts a
/// fresh worker and no crash is counted (PR #394 review).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_worker_that_died_idle_is_replaced_without_a_crash() {
    let store = TestStore::embedded(option_env!("CARGO_TARGET_TMPDIR")).await;
    let r = runner(&store).await;
    let handle = spawn(JsConfig {
        contexts: 1,
        ..JsConfig::default()
    })
    .await;
    let ok = handle.function("w:ok").expect("w:ok");
    assert_eq!(query(&r, &ok, unit()).await.expect("ok").result, s("fine"));
    let before = handle.pids();
    kill_workers(&handle);
    // SIGKILL is delivered asynchronously.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        query(&r, &ok, unit()).await.expect("a fresh worker").result,
        s("fine")
    );
    assert_ne!(before, handle.pids(), "a fresh worker process");
    assert_eq!(handle.crashes(), 0, "an idle death is not a crash");
}

/// Five crashes within a minute mark the deployment's workers degraded;
/// calls are still served, by workers respawned with backoff.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn five_crashes_in_a_minute_degrade_the_deployment() {
    let store = TestStore::embedded(option_env!("CARGO_TARGET_TMPDIR")).await;
    let r = runner(&store).await;
    let handle = spawn(JsConfig {
        contexts: 1,
        ..JsConfig::default()
    })
    .await;
    let ok = handle.function("w:ok").expect("w:ok");
    for crash in 1..=5u64 {
        assert!(!handle.degraded(), "degraded after {} crashes", crash - 1);
        // A probe that the sandbox kills (SIGSYS) is a crash like any other.
        expect_crash(
            handle.probe(loams_live_js::Probe::OpenFile).await,
            "a probe",
        );
        assert_eq!(handle.crashes(), crash);
        assert_eq!(
            query(&r, &ok, unit()).await.expect("respawned").result,
            s("fine")
        );
    }
    assert!(handle.degraded(), "five crashes in a minute degrade it");
    assert_eq!(
        query(&r, &ok, unit()).await.expect("still served").result,
        s("fine")
    );
    assert!(handle.degraded(), "degraded stays set");
}

/// `tenancy = "multi"` needs `isolation = "isolated"`, which exists on
/// Linux only; single tenancy takes either.
#[test]
fn multi_tenancy_requires_isolation() {
    let config = |tenancy, isolation| LiveConfig {
        tenancy,
        isolation,
        ..LiveConfig::new(std::path::Path::new("/data"), "app").expect("a valid app name")
    };
    let defaults = LiveConfig::new(std::path::Path::new("/data"), "app").expect("app");
    assert_eq!(defaults.tenancy, Tenancy::Single);
    assert_eq!(defaults.isolation, Isolation::InProcess);
    assert!(check_isolation(&defaults).is_ok());
    if cfg!(target_os = "linux") {
        assert!(check_isolation(&config(Tenancy::Single, Isolation::Isolated)).is_ok());
    }
    let refused = check_isolation(&config(Tenancy::Multi, Isolation::InProcess))
        .expect_err("multi tenancy in process is refused");
    assert_eq!(
        refused.to_string(),
        r#"live: tenancy = "multi" needs isolation = "isolated" (Linux only)"#
    );
    let multi = check_isolation(&config(Tenancy::Multi, Isolation::Isolated));
    if cfg!(target_os = "linux") {
        assert!(multi.is_ok(), "{multi:?}");
    } else {
        assert_eq!(
            multi.expect_err("no isolated mode off Linux").to_string(),
            r#"live: tenancy = "multi" needs isolation = "isolated" (Linux only)"#
        );
    }
}

/// LV1 row T3-10: a call stuck in a C built-in that never polls the
/// interrupt handler is killed from outside after `cpu_limit` plus the
/// grace, answers `FUNCTION_TIMEOUT`, and the next call runs on a fresh
/// worker. A kill for time is not a crash.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn stuck_builtin_is_killed_by_the_wall_clock() {
    let store = TestStore::embedded(option_env!("CARGO_TARGET_TMPDIR")).await;
    let r = runner(&store).await;
    let cpu = Duration::from_millis(200);
    let handle = spawn(JsConfig {
        cpu_limit: cpu,
        // Raised, so memory does not stop them first.
        memory_limit: 1 << 30,
        contexts: 1,
        ..JsConfig::default()
    })
    .await;
    let ok = handle.function("w:ok").expect("w:ok");
    for path in ["w:stuckStringify", "w:stuckRepeat"] {
        let before = handle.pids();
        let stuck = handle.function(path).expect(path);
        let started = Instant::now();
        match query(&r, &stuck, unit()).await {
            Err(e @ LiveError::FunctionTimeout { .. }) => {
                assert_eq!(e.code(), pb::ErrorCode::ERROR_CODE_FUNCTION_TIMEOUT);
                assert!(e.to_string().contains(path), "{e}");
            }
            other => panic!("{path}: a timeout, not {other:?}"),
        }
        let took = started.elapsed();
        assert!(
            took >= cpu + loams_live_js::KILL_GRACE,
            "{path}: killed after {took:?}, before cpu_limit plus the grace"
        );
        assert!(
            took < Duration::from_secs(5),
            "{path}: killed only after {took:?}"
        );
        let started = Instant::now();
        assert_eq!(
            query(&r, &ok, unit())
                .await
                .expect("the next call runs")
                .result,
            s("fine")
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{path}: the next call waited {:?}",
            started.elapsed()
        );
        assert_ne!(before, handle.pids(), "{path}: a fresh worker");
    }
    assert_eq!(handle.crashes(), 0, "a kill for time is not a crash");
}

/// Not a gate: the p50 cost of a trivial call and of one `ctx.db` round
/// trip, in process and isolated, printed for P8 (design §45 §15).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn host_call_latency_recorded() {
    let store = TestStore::embedded(option_env!("CARGO_TARGET_TMPDIR")).await;
    let r = runner(&store).await;
    let config = JsConfig {
        contexts: 1,
        ..JsConfig::default()
    };
    let in_process = load_with(BUNDLE, config.clone()).await;
    let isolated = spawn(config).await;
    let insert = in_process.function("w:insert").expect("w:insert");
    let id = mutate(&r, &insert, obj(&[("body", s("x"))]))
        .await
        .expect("insert")
        .result;
    const CALLS: usize = 200;
    const GETS: i64 = 50;
    let p50 = |mut v: Vec<Duration>| {
        v.sort();
        v[v.len() / 2]
    };
    let mut report = Vec::new();
    for (mode, ok, gets) in [
        (
            "in_process",
            in_process.function("w:ok").expect("ok"),
            in_process.function("w:gets").expect("gets"),
        ),
        (
            "isolated",
            isolated.function("w:ok").expect("ok"),
            isolated.function("w:gets").expect("gets"),
        ),
    ] {
        let at = r.store().now().await.expect("now");
        let mut trivial = Vec::with_capacity(CALLS);
        for _ in 0..CALLS {
            let started = Instant::now();
            r.query(&*ok, unit(), at).await.expect("ok");
            trivial.push(started.elapsed());
        }
        let mut per_get = Vec::with_capacity(CALLS / 10);
        let args = obj(&[("id", id.clone()), ("n", LiveValue::I64(GETS))]);
        for _ in 0..CALLS / 10 {
            let started = Instant::now();
            r.query(&*gets, args.clone(), at).await.expect("gets");
            per_get.push(started.elapsed() / GETS as u32);
        }
        report.push(format!(
            "{mode}: trivial call p50 {:?}, ctx.db.get round trip p50 {:?}",
            p50(trivial),
            p50(per_get)
        ));
    }
    println!("host_call_latency_recorded (P8, not a gate):");
    for line in report {
        println!("  {line}");
    }
}
