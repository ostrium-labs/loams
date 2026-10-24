#![allow(clippy::unwrap_used)]

//! MK1 Task 1: the listing schema accepts its valid fixtures, rejects each
//! invalid one, and agrees with `loams.marketplace.v1.Listing`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use loams_agentd_marketplace::listing::{self, schema};
use serde_json::Value;

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/marketplace/testdata")
}

fn listing_proto() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto/loams/marketplace/v1/listing.proto")
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn valid_fixtures() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(testdata().join("valid"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    paths
}

#[test]
fn listing_schema_accepts_fixtures() {
    let fixtures = valid_fixtures();
    // An agent (ACP and A2A), a harness (ACP and native) and an MCP server
    // (local and remote): one fixture per shape.
    assert!(fixtures.len() >= 6, "{fixtures:?}");
    let mut kinds = BTreeSet::new();
    for path in fixtures {
        let doc = read_json(&path);
        kinds.insert(doc["kind"].as_str().unwrap().to_owned());
        if let Err(issues) = listing::validate(&doc) {
            panic!("{} is rejected: {issues:#?}", path.display());
        }
    }
    assert_eq!(
        kinds,
        ["agent", "harness", "mcp-server"].map(String::from).into()
    );
}

/// Splits a JSON Pointer into its unescaped tokens.
fn tokens(pointer: &str) -> Vec<String> {
    pointer
        .split('/')
        .skip(1)
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect()
}

fn apply(doc: &mut Value, op: &Value) {
    let path = op["path"].as_str().unwrap();
    let tokens = tokens(path);
    let (last, parents) = tokens.split_last().expect("a non-root path");
    let mut target = &mut *doc;
    for token in parents {
        target = match target {
            Value::Array(items) => &mut items[token.parse::<usize>().unwrap()],
            Value::Object(map) => map.get_mut(token).unwrap_or_else(|| panic!("{path}")),
            _ => panic!("{path} goes through a scalar"),
        };
    }
    match (op["op"].as_str().unwrap(), target) {
        ("remove", Value::Object(map)) => {
            map.remove(last)
                .unwrap_or_else(|| panic!("{path} is absent"));
        }
        ("remove", Value::Array(items)) => {
            items.remove(last.parse::<usize>().unwrap());
        }
        ("set", Value::Object(map)) => {
            map.insert(last.clone(), op["value"].clone());
        }
        ("set", Value::Array(items)) => {
            items[last.parse::<usize>().unwrap()] = op["value"].clone();
        }
        (other, _) => panic!("unknown op {other} at {path}"),
    }
}

#[test]
fn listing_schema_rejects_each_invalid_fixture() {
    let manifest = read_json(&testdata().join("invalid.json"));
    let cases = manifest["cases"].as_array().unwrap();
    assert!(cases.len() >= 40, "every rule has a case");
    let mut names = BTreeSet::new();
    let mut accepted = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        assert!(names.insert(name), "duplicate case {name}");
        let base = testdata().join(case["base"].as_str().unwrap());
        let mut doc = read_json(&base);
        assert!(
            listing::validate(&doc).is_ok(),
            "{name}: base must be valid"
        );
        for op in case["patch"].as_array().unwrap() {
            apply(&mut doc, op);
        }
        if listing::validate(&doc).is_ok() {
            accepted.push(name);
        }
    }
    assert!(
        accepted.is_empty(),
        "accepted invalid listings: {accepted:?}"
    );
}

#[test]
fn issues_point_at_the_offending_value() {
    let mut doc = read_json(&testdata().join("valid/agent-mastra-acp.json"));
    doc["tools"][0]["risk"] = "low".into();
    let issues = listing::validate(&doc).unwrap_err();
    assert!(
        issues.iter().any(|i| i.instance_path == "/tools/0/risk"),
        "{issues:#?}"
    );
}

// --- proto_and_schema_agree -------------------------------------------------

#[derive(Debug, Clone)]
struct Field {
    ty: String,
    repeated: bool,
}

#[derive(Debug, Default)]
struct Proto {
    messages: BTreeMap<String, BTreeMap<String, Field>>,
    enums: BTreeMap<String, Vec<String>>,
}

/// A small reader for listing.proto: top-level `message` and `enum` blocks
/// with one field or value per line (the file's own style).
fn parse_proto(text: &str) -> Proto {
    let mut proto = Proto::default();
    let mut current: Option<(bool, String)> = None;
    for raw in text.lines() {
        let line = raw.split("//").next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("message ") {
            let name = rest.trim_end_matches(['{', '}', ' ']).trim().to_owned();
            proto.messages.entry(name.clone()).or_default();
            current = if line.ends_with('}') {
                None
            } else {
                Some((true, name))
            };
            continue;
        }
        if let Some(rest) = line.strip_prefix("enum ") {
            let name = rest.trim_end_matches(['{', ' ']).trim().to_owned();
            proto.enums.entry(name.clone()).or_default();
            current = Some((false, name));
            continue;
        }
        if line == "}" {
            current = None;
            continue;
        }
        let Some((is_message, name)) = &current else {
            continue;
        };
        let decl = line.trim_end_matches(';');
        let (lhs, _number) = decl.split_once('=').expect(line);
        let parts: Vec<&str> = lhs.split_whitespace().collect();
        if *is_message {
            let (repeated, ty, field) = match parts.as_slice() {
                ["repeated", ty, field] => (true, *ty, *field),
                [ty, field] => (false, *ty, *field),
                _ => panic!("unreadable field: {line}"),
            };
            proto.messages.get_mut(name).unwrap().insert(
                field.to_owned(),
                Field {
                    ty: ty.to_owned(),
                    repeated,
                },
            );
        } else {
            proto.enums.get_mut(name).unwrap().push(parts[0].to_owned());
        }
    }
    proto
}

/// Follows a local `$ref` once.
fn resolve<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
    match node.get("$ref").and_then(Value::as_str) {
        Some(reference) => {
            let pointer = reference.strip_prefix('#').expect("local $ref");
            root.pointer(pointer)
                .unwrap_or_else(|| panic!("{reference}"))
        }
        None => node,
    }
}

/// The proto type a (non-array) schema node must have.
fn expected_type(root: &Value, node: &Value) -> String {
    let node = resolve(root, node);
    if let Some(message) = node.get("x-proto-message").and_then(Value::as_str) {
        return message.to_owned();
    }
    if let Some(name) = node.get("x-proto-enum").and_then(Value::as_str) {
        return name.to_owned();
    }
    match node.get("type").and_then(Value::as_str) {
        Some("string") => "string".into(),
        Some("boolean") => "bool".into(),
        Some("integer") => "int".into(),
        Some("number") => "double".into(),
        Some("object") => "google.protobuf.Struct".into(),
        None => "google.protobuf.Value".into(),
        Some(other) => panic!("no proto type for {other}"),
    }
}

/// Every object schema with `x-proto-message`, and every enum with
/// `x-proto-enum`, anywhere in the schema.
fn walk<'a>(node: &'a Value, messages: &mut Vec<&'a Value>, enums: &mut Vec<&'a Value>) {
    match node {
        Value::Object(map) => {
            if map.contains_key("x-proto-message") {
                messages.push(node);
            }
            if map.contains_key("x-proto-enum") {
                enums.push(node);
            }
            for value in map.values() {
                walk(value, messages, enums);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, messages, enums);
            }
        }
        _ => {}
    }
}

fn upper_snake(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_uppercase());
    }
    out
}

#[test]
fn proto_and_schema_agree() {
    let text = std::fs::read_to_string(listing_proto()).unwrap();
    assert!(text.contains("package loams.marketplace.v1;"));
    let proto = parse_proto(&text);
    let root = schema();
    let mut messages = Vec::new();
    let mut enums = Vec::new();
    walk(root, &mut messages, &mut enums);

    let mut problems = Vec::new();
    let mut seen_messages = BTreeSet::new();
    for node in messages {
        let name = node["x-proto-message"].as_str().unwrap();
        seen_messages.insert(name.to_owned());
        let Some(fields) = proto.messages.get(name) else {
            problems.push(format!("message {name} is missing from listing.proto"));
            continue;
        };
        let properties = node["properties"].as_object().unwrap();
        let schema_names: BTreeSet<&str> = properties
            .keys()
            .map(String::as_str)
            .filter(|key| !key.starts_with('$'))
            .collect();
        let proto_names: BTreeSet<&str> = fields.keys().map(String::as_str).collect();
        for missing in schema_names.difference(&proto_names) {
            problems.push(format!("{name}.{missing} is in the schema, not the proto"));
        }
        for extra in proto_names.difference(&schema_names) {
            problems.push(format!("{name}.{extra} is in the proto, not the schema"));
        }
        for key in schema_names.intersection(&proto_names) {
            let field = &fields[*key];
            let property = resolve(root, &properties[*key]);
            let is_array = property.get("type").and_then(Value::as_str) == Some("array");
            if is_array != field.repeated {
                problems.push(format!(
                    "{name}.{key}: array in the schema iff repeated in the proto"
                ));
            }
            let element = if is_array {
                &property["items"]
            } else {
                property
            };
            let want = expected_type(root, element);
            let ok = match want.as_str() {
                "int" => field.ty == "int32" || field.ty == "int64",
                other => field.ty == other,
            };
            if !ok {
                problems.push(format!(
                    "{name}.{key}: proto type {} but the schema wants {want}",
                    field.ty
                ));
            }
        }
    }
    for name in proto.messages.keys() {
        if !seen_messages.contains(name) {
            problems.push(format!(
                "message {name} has no x-proto-message in the schema"
            ));
        }
    }

    let mut seen_enums = BTreeSet::new();
    for node in enums {
        let name = node["x-proto-enum"].as_str().unwrap();
        let values: Vec<String> = node["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} has no enum list"))
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        let prefix = format!("{}_", upper_snake(name));
        let mut want = vec![format!("{prefix}UNSPECIFIED")];
        want.extend(
            values
                .iter()
                .map(|v| format!("{prefix}{}", v.to_ascii_uppercase().replace('-', "_"))),
        );
        // An enum used in several places (McpTransport) is checked at each.
        seen_enums.insert(name.to_owned());
        match proto.enums.get(name) {
            Some(have) if *have == want => {}
            Some(have) => problems.push(format!(
                "enum {name}: proto {have:?}, schema wants {want:?}"
            )),
            None => problems.push(format!("enum {name} is missing from listing.proto")),
        }
    }
    for name in proto.enums.keys() {
        if !seen_enums.contains(name) {
            problems.push(format!("enum {name} has no x-proto-enum in the schema"));
        }
    }

    assert!(problems.is_empty(), "{problems:#?}");
}
