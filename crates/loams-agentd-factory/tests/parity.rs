//! Parity with the TypeScript ops (design §50 §13): every case in
//! `fixtures/parity.json` (written by `apps/desktop-electron/test/factory-parity.test.ts`
//! from the real adapters) is replayed here. The Rust op must send the same requests
//! (method, path, query in order, header names) and return the same value or the same
//! error code.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use loams_agentd_factory::ops::{self, Connection, OpContext};
use loams_agentd_factory::{AppId, OpError, Upstream, UpstreamRequest, UpstreamResponse};
use serde_json::{Value, json};

#[derive(Debug)]
struct Scripted {
    routes: Vec<Value>,
    sent: Mutex<Vec<Value>>,
}

#[async_trait]
impl Upstream for Scripted {
    async fn send(&self, request: UpstreamRequest) -> Result<UpstreamResponse, OpError> {
        let url = url::Url::parse(&request.url).unwrap();
        let query: Vec<Value> = url
            .query_pairs()
            .map(|(k, v)| json!([k.to_string(), v.to_string()]))
            .collect();
        let mut headers: Vec<String> = request.headers.iter().map(|(n, _)| n.clone()).collect();
        headers.sort();
        self.sent.lock().unwrap().push(json!({
            "method": request.method,
            "path": url.path(),
            "query": query,
            "headers": headers,
        }));
        let route = self.routes.iter().find(|r| {
            r["path"] == url.path()
                && r.get("method").and_then(Value::as_str).unwrap_or("GET") == request.method
        });
        let Some(route) = route else {
            return Ok(UpstreamResponse {
                status: 599,
                headers: vec![],
                body: b"no route".to_vec(),
            });
        };
        if route.get("network").and_then(Value::as_bool) == Some(true) {
            return Err(OpError::network("fetch failed"));
        }
        let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
        if let Some(Value::Object(extra)) = route.get("headers") {
            for (k, v) in extra {
                headers.push((k.to_ascii_lowercase(), v.as_str().unwrap_or("").to_owned()));
            }
        }
        Ok(UpstreamResponse {
            status: route.get("status").and_then(Value::as_u64).unwrap_or(200) as u16,
            headers,
            body: serde_json::to_vec(route.get("body").unwrap_or(&Value::Null)).unwrap(),
        })
    }
}

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/parity.json"
    ))
    .expect("tests/fixtures/parity.json");
    serde_json::from_str(&text).unwrap()
}

fn now_ms(fixture: &Value) -> i64 {
    // "2026-10-19T12:00:00.000Z"
    let now = fixture["now"].as_str().unwrap();
    assert_eq!(
        now, "2026-10-19T12:00:00.000Z",
        "update now_ms() with the fixture"
    );
    1_792_411_200_000
}

#[tokio::test]
async fn every_recorded_op_sends_the_same_requests_and_returns_the_same_value() {
    let fixture = fixture();
    let now = now_ms(&fixture);
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() >= 30, "the fixture covers every op");
    let mut failures = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let app = AppId::parse(case["app"].as_str().unwrap()).unwrap();
        let upstream = Arc::new(Scripted {
            routes: case["routes"].as_array().cloned().unwrap_or_default(),
            sent: Mutex::new(Vec::new()),
        });
        let values: BTreeMap<String, String> = case["fields"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
            .collect();
        let conn = Connection {
            url: case["url"].as_str().unwrap().to_owned(),
            values,
            upstream: upstream.clone(),
        };
        let op = case["op"].as_str().unwrap();
        let result = match ops::check(app, op, &case["params"]) {
            Err(e) => json!({ "ok": false, "code": e.code() }),
            Ok(params) => {
                match ops::run(app, op, &params, &conn, OpContext { now_ms: now }).await {
                    Ok(value) => json!({ "ok": true, "value": value }),
                    Err(e) => json!({ "ok": false, "code": e.code() }),
                }
            }
        };
        let sent = Value::Array(upstream.sent.lock().unwrap().clone());
        let want = &case["expect"];
        let want_requests: Vec<Value> = want["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                json!({
                    "method": r["method"], "path": r["path"], "query": r["query"], "headers": r["headers"],
                })
            })
            .collect();
        if sent != Value::Array(want_requests.clone()) {
            failures.push(format!(
                "{name}: requests differ\n  rust: {sent}\n  ts:   {}",
                Value::Array(want_requests)
            ));
        }
        if result != want["result"] {
            failures.push(format!(
                "{name}: result differs\n  rust: {result}\n  ts:   {}",
                want["result"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
