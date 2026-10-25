//! Native TiKV durable execution across two Resonate servers and a restart.
//! Runs when the standard TiKV playground is named by `LOAMS_TEST_PD`.
//!
//! Every test has a store of its own (a random root in the test keyspace),
//! and its own functions and counters: the tests share one process.
#![cfg(feature = "tikv")]

use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use loams_durable::{
    DurableConfig, DurableError, DurableRuntime, DurableServer, DurableStore, RuntimeOptions,
};
use loams_tikv::testing::{TEST_META, TestCluster, cluster};
use resonate_sdk::prelude::*;
use serde_json::{Value, json};

fn config(store: &DurableStore) -> DurableConfig {
    let probe = TcpListener::bind("127.0.0.1:0").expect("free loopback port");
    let mut config = DurableConfig::new(store.clone());
    config.listen = probe.local_addr().expect("local address");
    config
}

/// A store of its own: the test keyspace under a new random root.
fn store(cluster: &TestCluster) -> DurableStore {
    let tikv = cluster.config(TEST_META);
    DurableStore::Tikv {
        pd: tikv.pd,
        keyspace: tikv.keyspace,
        root: tikv.root,
    }
}

/// [`config`] with a short retry timeout, so a task a stopped runtime held
/// comes back within a test's patience.
fn quick(store: &DurableStore) -> DurableConfig {
    let mut config = config(store);
    config.retry_timeout = Duration::from_secs(1);
    config
}

/// A short lease, so a task held by a stopped runtime is handed out again
/// in seconds.
fn options() -> RuntimeOptions {
    RuntimeOptions {
        ttl: Duration::from_secs(2),
    }
}

/// Wait up to 60 s for `f`.
async fn within<F: std::future::IntoFuture>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(60), f.into_future())
        .await
        .expect("timed out")
}

/// Poll `cond` every 50 ms for up to 60 s.
async fn until(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after 1970")
            .as_millis(),
    )
    .expect("milliseconds fit i64")
}

async fn call(server: &DurableServer, kind: &str, data: Value) -> Value {
    server
        .process(json!({ "kind": kind, "data": data }))
        .await
        .unwrap_or_else(|e| panic!("{kind}: {e}"))
}

/// The status of a request that may be refused (a 404 is an answer here).
async fn status_of(server: &DurableServer, kind: &str, data: Value) -> i64 {
    match server.process(json!({ "kind": kind, "data": data })).await {
        Ok(response) => status(&response),
        Err(DurableError::Protocol { status, .. }) => i64::from(status),
        Err(e) => panic!("{kind}: {e}"),
    }
}

fn status(response: &Value) -> i64 {
    response["head"]["status"].as_i64().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn promise_state_survives_two_servers_and_restart() {
    let Some(cluster) = cluster().await else {
        return;
    };
    let store = store(&cluster);
    let first = DurableServer::start(config(&store), "durable-a")
        .await
        .expect("first server");
    let second = DurableServer::start(config(&store), "durable-b")
        .await
        .expect("second server");
    assert!(first.ready().await && second.ready().await);

    let created = first
        .process(json!({
            "kind": "promise.create",
            "data": {
                "id": "native-tikv-promise",
                "timeoutAt": i64::MAX / 2,
                "param": {},
                "tags": {}
            }
        }))
        .await
        .expect("promise created");
    assert_eq!(created["data"]["promise"]["state"], "pending");
    second
        .process(json!({
            "kind": "promise.settle",
            "data": {
                "id": "native-tikv-promise",
                "state": "resolved",
                "value": { "data": "YQ==" }
            }
        }))
        .await
        .expect("promise settled on another server");
    first.stop().await;
    second.stop().await;

    let restarted = DurableServer::start(config(&store), "durable-c")
        .await
        .expect("restarted server");
    let got = restarted
        .process(json!({
            "kind": "promise.get",
            "data": { "id": "native-tikv-promise" }
        }))
        .await
        .expect("read after restart");
    assert_eq!(got["data"]["promise"]["state"], "resolved");
    restarted.stop().await;
}

// ─── a function's tasks, and a sleep across a whole-server restart ─────────

static TWO_STEP_ONE: AtomicUsize = AtomicUsize::new(0);
static TWO_STEP_TWO: AtomicUsize = AtomicUsize::new(0);

#[resonate_sdk::function]
async fn tikv_step_one(x: i64) -> Result<i64> {
    TWO_STEP_ONE.fetch_add(1, Ordering::SeqCst);
    Ok(x + 1)
}

#[resonate_sdk::function]
async fn tikv_step_two(x: i64) -> Result<i64> {
    TWO_STEP_TWO.fetch_add(1, Ordering::SeqCst);
    Ok(x * 10)
}

#[resonate_sdk::function]
async fn tikv_two_step(ctx: &Context, x: i64) -> Result<i64> {
    let a: i64 = ctx.run(tikv_step_one, x).await?;
    ctx.run(tikv_step_two, a).await
}

fn register_two_step(sdk: &Resonate) -> Result<()> {
    sdk.register(tikv_two_step)?;
    sdk.register(tikv_step_one)?;
    sdk.register(tikv_step_two)
}

/// Loams's own runtime on the TiKV store: the root's task is created,
/// acquired and fulfilled through the in-process worker, and a second run of
/// the same id is the memoized result, even from a server started afresh.
#[tokio::test(flavor = "multi_thread")]
async fn function_tasks_round_trip_and_memoize_across_restart() {
    let Some(cluster) = cluster().await else {
        return;
    };
    let store = store(&cluster);
    let server = DurableServer::start(quick(&store), "tikv-fn-1")
        .await
        .expect("server");
    let runtime = DurableRuntime::start_with(&server, "tikv-fn-1", options(), register_two_step)
        .await
        .expect("runtime");
    let result: i64 = within(runtime.sdk().run("tikv-two-step", tikv_two_step, 4_i64))
        .await
        .expect("the result");
    assert_eq!(result, 50, "(4 + 1) * 10");
    assert_eq!(TWO_STEP_ONE.load(Ordering::SeqCst), 1);
    assert_eq!(TWO_STEP_TWO.load(Ordering::SeqCst), 1);

    // The root's task is settled in the store, and no task is left pending.
    let tasks = call(
        &server,
        "task.search",
        json!({ "state": "pending", "limit": 10 }),
    )
    .await;
    assert_eq!(status(&tasks), 200, "{tasks}");
    assert_eq!(tasks["data"]["tasks"], json!([]), "{tasks}");
    runtime.stop().await;
    server.stop().await;

    let restarted = DurableServer::start(quick(&store), "tikv-fn-2")
        .await
        .expect("restarted server");
    let root = call(&restarted, "promise.get", json!({ "id": "tikv-two-step" })).await;
    assert_eq!(root["data"]["promise"]["state"], "resolved", "{root}");
    let runtime = DurableRuntime::start_with(&restarted, "tikv-fn-2", options(), register_two_step)
        .await
        .expect("runtime after restart");
    let again: i64 = within(runtime.sdk().run("tikv-two-step", tikv_two_step, 4_i64))
        .await
        .expect("the memoized result");
    assert_eq!(again, 50);
    assert_eq!(TWO_STEP_ONE.load(Ordering::SeqCst), 1, "not run again");
    assert_eq!(TWO_STEP_TWO.load(Ordering::SeqCst), 1, "not run again");
    runtime.stop().await;
    restarted.stop().await;
}

static BEFORE_SLEEP: AtomicUsize = AtomicUsize::new(0);
static AFTER_SLEEP: AtomicUsize = AtomicUsize::new(0);

#[resonate_sdk::function]
async fn tikv_note_before() -> Result<()> {
    BEFORE_SLEEP.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[resonate_sdk::function]
async fn tikv_note_after() -> Result<()> {
    AFTER_SLEEP.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[resonate_sdk::function]
async fn tikv_sleepy(ctx: &Context) -> Result<u64> {
    ctx.run(tikv_note_before, ()).await?;
    ctx.sleep(Duration::from_secs(3)).await?;
    ctx.run(tikv_note_after, ()).await?;
    Ok(7)
}

fn register_sleepy(sdk: &Resonate) -> Result<()> {
    sdk.register(tikv_sleepy)?;
    sdk.register(tikv_note_before)?;
    sdk.register(tikv_note_after)
}

/// A workflow suspended on a timer, with the whole server stopped (the
/// engine restarting): a new server on the same store wakes it from TiKV and
/// it finishes without running its first step again.
#[tokio::test(flavor = "multi_thread")]
async fn suspended_workflow_survives_a_server_restart() {
    let Some(cluster) = cluster().await else {
        return;
    };
    let store = store(&cluster);
    let started = Instant::now();
    let server = DurableServer::start(quick(&store), "tikv-sleep-1")
        .await
        .expect("server");
    let runtime = DurableRuntime::start_with(&server, "tikv-sleep-1", options(), register_sleepy)
        .await
        .expect("runtime");
    let _handle = within(runtime.sdk().run("tikv-sleepy", tikv_sleepy, ()).spawn())
        .await
        .expect("started");
    until("the step before the sleep", || {
        BEFORE_SLEEP.load(Ordering::SeqCst) == 1
    })
    .await;
    // The counter moves before the step's promise is settled: wait until the
    // store has it resolved (this test's root holds no other resolved
    // promise before the sleep ends), so stopping cannot re-run the step.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let resolved = call(
            &server,
            "promise.search",
            json!({ "state": "resolved", "limit": 10 }),
        )
        .await;
        if resolved["data"]["promises"]
            .as_array()
            .is_some_and(|p| !p.is_empty())
        {
            break;
        }
        assert!(Instant::now() < deadline, "the first step never settled");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    runtime.stop().await;
    server.stop().await;
    assert_eq!(AFTER_SLEEP.load(Ordering::SeqCst), 0, "still asleep");

    let server = DurableServer::start(quick(&store), "tikv-sleep-2")
        .await
        .expect("restarted server");
    let runtime = DurableRuntime::start_with(&server, "tikv-sleep-2", options(), register_sleepy)
        .await
        .expect("runtime after restart");
    let handle = within(runtime.sdk().get::<u64>("tikv-sleepy"))
        .await
        .expect("a handle");
    let result = within(handle.result()).await.expect("the result");
    assert_eq!(result, 7);
    assert!(started.elapsed() >= Duration::from_secs(3));
    assert_eq!(BEFORE_SLEEP.load(Ordering::SeqCst), 1, "not replayed");
    assert_eq!(AFTER_SLEEP.load(Ordering::SeqCst), 1);
    runtime.stop().await;
    server.stop().await;
}

// ─── schedules ─────────────────────────────────────────────────────────────

/// The schedule calls the console's durable client makes (create, get,
/// search, delete), and a schedule created on one server fires from another
/// started after the first stopped: its timer is re-armed from TiKV.
#[tokio::test(flavor = "multi_thread")]
async fn schedules_persist_and_fire_after_a_restart() {
    let Some(cluster) = cluster().await else {
        return;
    };
    let store = store(&cluster);
    let first = DurableServer::start(config(&store), "tikv-sched-1")
        .await
        .expect("server");
    let created = call(
        &first,
        "schedule.create",
        json!({
            "id": "tikv-every-minute",
            "cron": "* * * * *",
            "promiseId": "tikv-tick-{{.id}}-{{.timestamp}}",
            "promiseTimeout": 3_600_000,
            "promiseParam": {},
            "promiseTags": { "resonate:target": "poll://any@tikv-schedule-test" }
        }),
    )
    .await;
    assert!(matches!(status(&created), 200 | 201), "{created}");
    let due = created["data"]["schedule"]["nextRunAt"]
        .as_i64()
        .expect("nextRunAt");
    let again = call(
        &first,
        "schedule.create",
        json!({
            "id": "tikv-every-minute",
            "cron": "* * * * *",
            "promiseId": "tikv-tick-{{.id}}-{{.timestamp}}",
            "promiseTimeout": 3_600_000,
            "promiseParam": {},
            "promiseTags": { "resonate:target": "poll://any@tikv-schedule-test" }
        }),
    )
    .await;
    assert_eq!(
        status(&again),
        200,
        "creating it again is idempotent: {again}"
    );
    let doomed = call(
        &first,
        "schedule.create",
        json!({
            "id": "tikv-deleted",
            "cron": "0 0 1 1 *",
            "promiseId": "tikv-never-{{.timestamp}}",
            "promiseTimeout": 60_000,
            "promiseParam": {},
            "promiseTags": { "resonate:target": "poll://any@tikv-schedule-test" }
        }),
    )
    .await;
    assert!(matches!(status(&doomed), 200 | 201), "{doomed}");
    let deleted = call(&first, "schedule.delete", json!({ "id": "tikv-deleted" })).await;
    assert_eq!(status(&deleted), 200, "{deleted}");
    first.stop().await;

    let second = DurableServer::start(config(&store), "tikv-sched-2")
        .await
        .expect("restarted server");
    let got = call(
        &second,
        "schedule.get",
        json!({ "id": "tikv-every-minute" }),
    )
    .await;
    assert_eq!(status(&got), 200, "{got}");
    assert_eq!(got["data"]["schedule"]["cron"], "* * * * *");
    let gone = status_of(&second, "schedule.get", json!({ "id": "tikv-deleted" })).await;
    assert_eq!(gone, 404, "the deleted schedule stays deleted");
    let listed = call(&second, "schedule.search", json!({ "limit": 10 })).await;
    assert_eq!(status(&listed), 200, "{listed}");
    let ids: Vec<&str> = listed["data"]["schedules"]
        .as_array()
        .expect("schedules")
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
    assert_eq!(ids, ["tikv-every-minute"], "{listed}");

    // The occurrence at `due` fires from the second server's timer.
    let tick = format!("tikv-tick-tikv-every-minute-{due}");
    let deadline = Instant::now()
        + Duration::from_millis(u64::try_from((due - now_ms()).max(0)).unwrap_or(0))
        + Duration::from_secs(30);
    loop {
        if status_of(&second, "promise.get", json!({ "id": tick })).await == 200 {
            let promise = call(&second, "promise.get", json!({ "id": tick })).await;
            assert_eq!(
                promise["data"]["promise"]["tags"]["resonate:schedule"],
                "tikv-every-minute"
            );
            break;
        }
        assert!(Instant::now() < deadline, "{tick} never fired");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    // The occurrence's promise is written before the schedule moves on (a
    // crash in between refires the same, idempotent, occurrence), so the
    // move can trail the promise by a write or two.
    let moved_by = Instant::now() + Duration::from_secs(10);
    let moved = loop {
        let moved = call(
            &second,
            "schedule.get",
            json!({ "id": "tikv-every-minute" }),
        )
        .await;
        if moved["data"]["schedule"]["nextRunAt"].as_i64() > Some(due) || Instant::now() > moved_by
        {
            break moved;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(
        moved["data"]["schedule"]["nextRunAt"].as_i64() > Some(due),
        "{moved}"
    );
    let deleted = call(
        &second,
        "schedule.delete",
        json!({ "id": "tikv-every-minute" }),
    )
    .await;
    assert_eq!(status(&deleted), 200, "{deleted}");
    second.stop().await;
}
