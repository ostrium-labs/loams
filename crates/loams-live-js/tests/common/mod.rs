//! Helpers shared by the `loams-live-js` suites.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use loams_live::testing::TestStore;
use loams_live::{
    AppKeys, Function, Limits, LiveError, LiveTxn, LiveValue, Mutated, Queried, Runner,
};
use loams_live_js::{Bundle, Isolation, JsConfig, WorkerCommand, WorkerPool};

pub fn s(v: &str) -> LiveValue {
    LiveValue::Str(v.to_string())
}

pub fn obj(pairs: &[(&str, LiveValue)]) -> LiveValue {
    LiveValue::Object(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
}

pub fn field(v: &LiveValue, name: &str) -> LiveValue {
    match v {
        LiveValue::Object(f) => f.get(name).cloned().unwrap_or(LiveValue::Null),
        other => panic!("an object, not {other:?}"),
    }
}

pub fn items(v: &LiveValue) -> Vec<LiveValue> {
    match v {
        LiveValue::Array(items) => items.clone(),
        other => panic!("an array, not {other:?}"),
    }
}

pub async fn load(source: &str) -> Bundle {
    load_with(source, JsConfig::default()).await
}

pub async fn load_with(source: &str, config: JsConfig) -> Bundle {
    match try_load(source, config).await {
        Ok(b) => b,
        Err(e) => panic!("the bundle loads: {e}"),
    }
}

tokio::task_local! {
    /// Set inside [`isolated`] and [`isolated_sync`]: the suites' bundles
    /// run in isolated workers (LV1 plan Task 5,
    /// `functions_suite_runs_isolated`).
    static ISOLATED: bool;
}

/// Whether the running test loads its bundles into isolated workers.
pub fn is_isolated() -> bool {
    ISOLATED.try_with(|i| *i).unwrap_or(false)
}

/// Runs a test body with its bundles in isolated workers.
pub async fn isolated<F: std::future::Future<Output = ()>>(body: F) {
    ISOLATED.scope(true, body).await;
}

/// Runs a synchronous test (a `#[test]`, or a `#[tokio::test]`, which
/// polls its body on this thread) with its bundles in isolated workers.
pub fn isolated_sync(test: impl FnOnce()) {
    ISOLATED.sync_scope(true, test);
}

/// The worker processes of the isolated suites: this crate's own worker
/// binary, which runs what `loams live-worker` runs.
pub fn workers() -> WorkerPool {
    WorkerPool::new(WorkerCommand::new(env!("CARGO_BIN_EXE_loams-live-worker")))
}

/// Loads `source` with `config`, in process or, inside [`isolated`], in an
/// isolated worker.
pub async fn try_load(source: &str, mut config: JsConfig) -> Result<Bundle, LiveError> {
    if is_isolated() {
        config.isolation = Isolation::Isolated;
        workers()
            .spawn("test", source.as_bytes(), config)
            .await
            .map(Bundle::from)
    } else {
        Bundle::load(source, config).await
    }
}

pub fn function(bundle: &Bundle, path: &str) -> Arc<dyn Function> {
    bundle
        .function(path)
        .unwrap_or_else(|| panic!("{path} is exported"))
}

pub async fn runner(store: &TestStore) -> Runner {
    Runner::open(store.store(), &store.live_config("js"))
        .await
        .expect("the runner opens")
}

pub async fn query(
    r: &Runner,
    f: &Arc<dyn Function>,
    args: LiveValue,
) -> Result<Queried, LiveError> {
    let at = r.store().now().await.expect("now");
    r.query(&**f, args, at).await
}

pub async fn mutate(
    r: &Runner,
    f: &Arc<dyn Function>,
    args: LiveValue,
) -> Result<Mutated, LiveError> {
    r.mutate(f.clone(), args, None).await
}

/// Runs query `f` at `at` with `request_id`, outside the runner, which
/// gives queries no request id.
pub async fn query_as(
    r: &Runner,
    f: &Arc<dyn Function>,
    args: LiveValue,
    at: loams_kv::Ts,
    request_id: &str,
) -> Result<LiveValue, LiveError> {
    let mut snap = r.store().snapshot(at).await.expect("a snapshot");
    let app = AppKeys::dedicated();
    let limits = Limits::default();
    let mut txn = LiveTxn::for_query(&mut snap, &app, &limits);
    txn.set_ctx(loams_live::CallCtx {
        request_id: request_id.to_string(),
        ..Default::default()
    });
    f.call(&mut txn, args).await
}

pub fn unit() -> LiveValue {
    obj(&[])
}

/// The variable that marks a child test process, holding the id of the
/// test it runs.
const CHILD_ENV: &str = "LOAMS_LIVE_JS_CHILD";

/// Whether this process is the child [`run_child`] started for `test`.
pub fn is_child(test: &str) -> bool {
    std::env::var(CHILD_ENV).is_ok_and(|v| v == test)
}

/// Runs the test `test` (its full path in this test binary) in a child
/// process with `envs` set, and returns its output. A test that could
/// abort the process or allocate without bound runs its body this way, so
/// a regression fails the test instead of killing the runner. Panics,
/// with the child's output, unless exactly that test ran and passed.
pub fn run_child(test: &str, envs: &[(&str, &str)]) -> String {
    let exe = std::env::current_exe().expect("the test binary");
    let out = std::process::Command::new(exe)
        .args([test, "--exact", "--test-threads=1", "--nocapture"])
        .env(CHILD_ENV, test)
        .envs(envs.iter().copied())
        .output()
        .expect("the child test runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success() && text.contains("1 passed"),
        "the child {test} ({envs:?}) failed: {}\n{text}",
        out.status
    );
    text
}

/// The full path of test `name`'s `store` case in this binary, for
/// [`run_child`]: under [`isolated`], the case of the suite's
/// `functions_suite_runs_isolated` module.
pub fn case_path(name: &str, store: Option<&TestStore>) -> String {
    let mut path = String::new();
    if is_isolated() {
        path.push_str("functions_suite_runs_isolated::");
    }
    path.push_str(name);
    if let Some(store) = store {
        path.push_str("::");
        path.push_str(variant(store));
    }
    path
}

/// The `live_test!` variant name of `store`'s backend.
pub fn variant(store: &TestStore) -> &'static str {
    match store.backend() {
        loams_kv::Backend::Embedded => "embedded",
        #[allow(unreachable_patterns)]
        _ => "tikv",
    }
}
