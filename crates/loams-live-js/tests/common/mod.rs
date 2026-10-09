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

pub fn load(source: &str) -> Bundle {
    load_with(source, JsConfig::default())
}

pub fn load_with(source: &str, config: JsConfig) -> Bundle {
    match Bundle::load(source, config) {
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
    txn.set_request_id(request_id);
    f.call(&mut txn, args).await
}

pub fn unit() -> LiveValue {
    obj(&[])
}
