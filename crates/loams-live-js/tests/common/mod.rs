//! Helpers shared by the `loams-live-js` suites.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use loams_live::testing::TestStore;
use loams_live::{
    AppKeys, Function, Limits, LiveError, LiveTxn, LiveValue, Mutated, Queried, Runner,
};
use loams_live_js::{Bundle, JsConfig};

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
    match Bundle::load(source, config).await {
        Ok(b) => b,
        Err(e) => panic!("the bundle loads: {e}"),
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

/// The `live_test!` variant name of `store`'s backend.
pub fn variant(store: &TestStore) -> &'static str {
    match store.backend() {
        loams_kv::Backend::Embedded => "embedded",
        #[allow(unreachable_patterns)]
        _ => "tikv",
    }
}
