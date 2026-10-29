//! The managed-engines contract (`api/engines/openapi.json`, design 81 D2015),
//! the seed's `engines` key and [`engine_routes`] agree.

use std::collections::BTreeSet;
use std::time::SystemTime;

use httpmock::MockServer;
use loams_console_mock::{ENGINES_CONTRACT, Route, engine_routes, load_seed, register};
use serde_json::Value;

fn contract() -> Value {
    serde_json::from_str(ENGINES_CONTRACT).expect("the engines contract is JSON")
}

fn all_routes() -> Vec<Route> {
    let seed = load_seed(SystemTime::now()).expect("the seed loads");
    engine_routes(&seed).expect("engine routes build")
}

#[test]
fn every_operation_has_a_mock_and_every_mock_an_operation() {
    let contract = contract();
    let mut ops = BTreeSet::new();
    for (path, item) in contract["paths"].as_object().expect("paths") {
        for method in item.as_object().expect("path item").keys() {
            ops.insert((method.to_uppercase(), path.clone()));
        }
    }
    let mocked: BTreeSet<(String, String)> = all_routes()
        .into_iter()
        .map(|r| (r.method.to_string(), r.template.to_string()))
        .collect();
    let missing: Vec<_> = ops.difference(&mocked).collect();
    let extra: Vec<_> = mocked.difference(&ops).collect();
    assert!(
        missing.is_empty(),
        "engines operations without a mock: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "mocks for paths the contract lacks: {extra:?}"
    );
}

#[test]
fn every_mocked_body_matches_its_response_schema() {
    let contract = contract();
    let mut checked = 0;
    for route in all_routes() {
        let op = &contract["paths"][route.template][route.method.to_lowercase()];
        assert!(
            !op.is_null(),
            "{} {} is not in the contract",
            route.method,
            route.template
        );
        let schema =
            &op["responses"][route.status.to_string()]["content"]["application/json"]["schema"];
        match (&route.body, schema.is_null()) {
            (None, true) => {}
            (Some(body), false) => {
                let mut errors = Vec::new();
                validate(&contract, schema, body, "$", &mut errors);
                assert!(
                    errors.is_empty(),
                    "{} {} ({}): {}",
                    route.method,
                    route.path,
                    route.status,
                    errors.join("; ")
                );
                checked += 1;
            }
            (body, _) => panic!(
                "{} {} ({}): body {} but the contract {} a JSON response",
                route.method,
                route.path,
                route.status,
                if body.is_some() { "present" } else { "absent" },
                if schema.is_null() {
                    "has no"
                } else {
                    "declares"
                },
            ),
        }
    }
    assert!(checked > 50, "only {checked} bodies checked");
}

#[test]
fn every_seeded_instance_is_preview_and_in_a_seeded_namespace() {
    let seed = load_seed(SystemTime::now()).unwrap();
    let namespaces: BTreeSet<&str> = seed["environments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["namespace"].as_str().unwrap())
        .collect();
    let instances = seed["engines"]["instances"].as_array().unwrap();
    let kinds: BTreeSet<&str> = instances
        .iter()
        .map(|i| i["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, BTreeSet::from(["flink", "risingwave", "spark"]));
    for i in instances {
        assert_eq!(i["stability"], "preview", "{}", i["id"]);
        assert!(
            namespaces.contains(i["namespace"].as_str().unwrap()),
            "{} is in an unseeded namespace",
            i["id"]
        );
        assert!(
            i.get("project").is_none(),
            "{} still names a project",
            i["id"]
        );
    }
    // Usage is integers (design 63), and a running instance's first window is estimated.
    for (id, usage) in seed["engines"]["usage"].as_object().unwrap() {
        for s in usage["samples"].as_array().unwrap() {
            assert!(s["cpu_ms"].is_u64(), "{id}: cpu_ms is not an integer");
            assert!(
                s["memory_mib_ms"].is_u64(),
                "{id}: memory_mib_ms is not an integer"
            );
            assert!(
                s["estimated"].is_boolean(),
                "{id}: estimated is not a boolean"
            );
        }
    }
}

#[tokio::test]
async fn the_server_answers_engines_from_the_seed() {
    let seed = load_seed(SystemTime::now()).unwrap();
    let server = MockServer::start_async().await;
    register(&server, &engine_routes(&seed).unwrap())
        .await
        .unwrap();
    let client = reqwest::Client::new();

    let all: Value = client
        .get(server.url("/v1/namespaces/code-index-development/engines"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(all["instances"].as_array().unwrap().len(), 3);

    // The kind filter matches before the unfiltered mock.
    let spark: Value = client
        .get(server.url("/v1/namespaces/code-index-development/engines?kind=spark"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let spark = spark["instances"].as_array().unwrap();
    assert_eq!(spark.len(), 2);
    assert!(spark.iter().all(|i| i["kind"] == "spark"));

    let suspended: Value = client
        .post(server.url("/v1/namespaces/docs-search-production/engines/eng_01docsrw/suspend"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(suspended["status"]["phase"], "suspending");

    let status = client
        .delete(
            server
                .url("/v1/namespaces/code-index-development/engines/eng_04codesp/sessions/ses_01"),
        )
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 204);
}

/// The JSON Schema subset the engines contract uses: `$ref`, `type` (one or a
/// list), `enum`, `properties`, `required`, `additionalProperties: false`,
/// `items`, `anyOf`/`oneOf`, `minimum` and `maximum` (as `tests/contract.rs`,
/// plus `oneOf`).
fn validate(root: &Value, schema: &Value, value: &Value, at: &str, errors: &mut Vec<String>) {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let name = r.trim_start_matches("#/components/schemas/");
        let target = &root["components"]["schemas"][name];
        assert!(!target.is_null(), "unknown $ref {r}");
        return validate(root, target, value, at, errors);
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(any) = schema.get(key).and_then(Value::as_array) {
            let matching = any
                .iter()
                .filter(|s| {
                    let mut e = Vec::new();
                    validate(root, s, value, at, &mut e);
                    e.is_empty()
                })
                .count();
            if matching == 0 || (key == "oneOf" && matching > 1) {
                errors.push(format!("{at}: matches {matching} of {key}"));
            }
            return;
        }
    }
    if let Some(t) = schema.get("type") {
        let types: Vec<&str> = match t {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        let actual = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        let ok = types.contains(&actual) || (actual == "integer" && types.contains(&"number"));
        if !ok {
            errors.push(format!("{at}: expected {types:?}, got {actual}"));
            return;
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        errors.push(format!("{at}: {value} is not one of {allowed:?}"));
    }
    if let Some(n) = value.as_f64()
        && let Some(min) = schema.get("minimum").and_then(Value::as_f64)
        && n < min
    {
        errors.push(format!("{at}: {n} < minimum {min}"));
    }
    if let Some(n) = value.as_f64()
        && let Some(max) = schema.get("maximum").and_then(Value::as_f64)
        && n > max
    {
        errors.push(format!("{at}: {n} > maximum {max}"));
    }
    if let Value::Object(map) = value {
        let props = schema.get("properties").and_then(Value::as_object);
        for key in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let key = key.as_str().unwrap_or_default();
            if !map.contains_key(key) {
                errors.push(format!("{at}: missing required {key}"));
            }
        }
        for (key, v) in map {
            match props.and_then(|p| p.get(key)) {
                Some(s) => validate(root, s, v, &format!("{at}.{key}"), errors),
                None => match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => {
                        errors.push(format!("{at}: unexpected property {key}"));
                    }
                    Some(extra @ Value::Object(_)) => {
                        validate(root, extra, v, &format!("{at}.{key}"), errors);
                    }
                    _ => {}
                },
            }
        }
    }
    if let (Value::Array(list), Some(item)) = (value, schema.get("items")) {
        for (i, v) in list.iter().enumerate() {
            validate(root, item, v, &format!("{at}[{i}]"), errors);
        }
    }
}
