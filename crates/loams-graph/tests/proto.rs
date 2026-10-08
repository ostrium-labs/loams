//! `loams.graph.v1` as reworked by GR1 Task 2 (design §48 §8, D746): the descriptor carries no
//! client-chosen path, and `value.rs` maps every value Grafeo has to and from the wire without
//! loss.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use grafeo::Value;
use grafeo_common::types::{Date, Duration, PropertyKey, Time, Timestamp, ZonedDatetime};
use loams_graph::value::{from_proto, to_proto};
use loams_graph::{Engine, Graph, OpenSpec};
use loams_proto::loams::graph::v1 as pb;
use pb::__buffa::oneof::value::Kind;
use prost::Message;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn round_trip(value: &Value) -> Value {
    from_proto(&to_proto(value)).unwrap_or_else(|err| panic!("{value:?} came back as {err}"))
}

fn map(entries: &[(&str, Value)]) -> Value {
    let mut out = BTreeMap::new();
    for (key, value) in entries {
        out.insert(PropertyKey::new(*key), value.clone());
    }
    Value::Map(Arc::new(out))
}

fn list(items: Vec<Value>) -> Value {
    Value::List(Arc::from(items))
}

fn s(text: &str) -> Value {
    Value::String(text.into())
}

fn proto(kind: Kind) -> pb::Value {
    pb::Value {
        kind: Some(kind),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------------------------
// The descriptor
// ---------------------------------------------------------------------------------------------

/// Review Focus 4: no RPC field can name a server path or a URL. The two fields that name an
/// object are keys under the namespace's own prefix, which the server resolves.
///
/// A field is suspect when one of the words of its name (split on `_`) is a path, URL, directory
/// or file word. Words rather than substrings, because `profile` (GQL's PROFILE) contains `file`;
/// and two fields are graph paths, not filesystem paths: `Value.path` (the GQL PATH type) and
/// `GraphLimits.max_path_hops`.
#[test]
fn proto_has_no_path_fields() {
    const ALLOWED: &[&str] = &[
        "loams.graph.v1.ImportGraphRequest.object_key",
        "loams.graph.v1.ExportGraphRequest.object_prefix",
        "loams.graph.v1.Value.path",
        "loams.graph.v1.GraphLimits.max_path_hops",
    ];
    const SUSPECT: &[&str] = &[
        "path",
        "paths",
        "filepath",
        "url",
        "urls",
        "uri",
        "uris",
        "dir",
        "dirs",
        "directory",
        "file",
        "files",
        "filename",
        "folder",
        "location",
    ];
    let set = prost_types::FileDescriptorSet::decode(loams_proto::FILE_DESCRIPTOR_SET)
        .expect("loams-proto's descriptor set decodes");
    let file = set
        .file
        .iter()
        .find(|f| f.package() == "loams.graph.v1")
        .expect("loams.graph.v1 is in loams-proto's descriptor set");

    fn walk(prefix: &str, message: &prost_types::DescriptorProto, out: &mut Vec<String>) {
        let name = format!("{prefix}.{}", message.name());
        for field in &message.field {
            out.push(format!("{name}.{}", field.name()));
        }
        for nested in &message.nested_type {
            walk(&name, nested, out);
        }
    }
    let mut fields = Vec::new();
    for message in &file.message_type {
        walk("loams.graph.v1", message, &mut fields);
    }
    assert!(
        fields.len() > 100,
        "the walk found only {} fields",
        fields.len()
    );
    for allowed in ALLOWED {
        assert!(fields.iter().any(|f| f == allowed), "{allowed} is missing");
    }
    // The fabric-era field this rule exists for would be caught.
    assert!(
        "database_path"
            .split('_')
            .any(|word| SUSPECT.contains(&word))
    );
    let offending: Vec<&String> = fields
        .iter()
        .filter(|f| !ALLOWED.contains(&f.as_str()))
        .filter(|f| {
            let leaf = f.rsplit('.').next().unwrap_or_default();
            leaf.split('_').any(|word| SUSPECT.contains(&word))
        })
        .collect();
    assert!(
        offending.is_empty(),
        "fields that could name a path: {offending:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------------------------

#[test]
fn int64_round_trips_exactly() {
    for n in [
        i64::MAX,
        i64::MIN,
        (1_i64 << 53) + 1,
        -((1_i64 << 53) + 1),
        0,
        -1,
    ] {
        let wire = to_proto(&Value::Int64(n));
        assert!(
            matches!(wire.kind, Some(Kind::Int64(m)) if m == n),
            "{n}: {:?}",
            wire.kind.is_some()
        );
        assert_eq!(round_trip(&Value::Int64(n)), Value::Int64(n));
        // proto3 JSON writes a 64-bit integer as a decimal string, which is what keeps 2^53 + 1
        // exact in JavaScript.
        let json = serde_json::to_value(&wire).expect("serializes");
        assert_eq!(json, serde_json::json!({ "int64": n.to_string() }));
        let back: pb::Value = serde_json::from_value(json).expect("deserializes");
        assert_eq!(from_proto(&back).expect("decodes"), Value::Int64(n));
    }
}

/// Grafeo has no unsigned integer. A `uint64` that fits is stored as `INT64` exactly; one that
/// does not is refused rather than wrapped.
#[test]
fn uint64_round_trips() {
    for n in [0_u64, 1, (1 << 53) + 1, i64::MAX as u64] {
        let value = from_proto(&proto(Kind::Uint64(n))).expect("fits in INT64");
        assert_eq!(value, Value::Int64(n as i64));
        let wire = to_proto(&value);
        assert!(matches!(wire.kind, Some(Kind::Int64(m)) if m as u64 == n));
    }
    for n in [i64::MAX as u64 + 1, u64::MAX] {
        let err = from_proto(&proto(Kind::Uint64(n))).expect_err("does not fit in INT64");
        assert!(err.to_string().contains("uint64"), "{err}");
    }
}

#[test]
fn temporal_values_round_trip() {
    let date = Date::from_ymd(2026, 10, 8).expect("a date");
    let ancient = Date::from_ymd(-44, 3, 15).expect("a BCE date");
    let local_time = Time::from_hms_nano(23, 59, 58, 123_456_789).expect("a time");
    let zoned_time = Time::from_hms_nano(7, 30, 0, 5)
        .expect("a time")
        .with_offset(-5 * 3600);
    let timestamp = Timestamp::from_micros(1_791_504_000_123_456);
    let before_epoch = Timestamp::from_micros(-1_234_567);
    let zoned = ZonedDatetime::from_timestamp_offset(timestamp, 5 * 3600 + 1800);
    let zoned_negative = ZonedDatetime::from_timestamp_offset(before_epoch, -9 * 3600);
    let duration = Duration::new(14, -3, 1_000_000_007);

    for value in [
        Value::Date(date),
        Value::Date(ancient),
        Value::Time(local_time),
        Value::Time(zoned_time),
        Value::Timestamp(timestamp),
        Value::Timestamp(before_epoch),
        Value::ZonedDatetime(zoned),
        Value::ZonedDatetime(zoned_negative),
        Value::Duration(duration),
    ] {
        assert_eq!(round_trip(&value), value);
    }

    // Each lands on its own wire type.
    assert!(matches!(
        to_proto(&Value::Date(date)).kind,
        Some(Kind::Date(_))
    ));
    assert!(matches!(
        to_proto(&Value::Time(local_time)).kind,
        Some(Kind::LocalTime(_))
    ));
    assert!(matches!(
        to_proto(&Value::Time(zoned_time)).kind,
        Some(Kind::ZonedTime(_))
    ));
    assert!(matches!(
        to_proto(&Value::Timestamp(timestamp)).kind,
        Some(Kind::LocalDatetime(_))
    ));
    assert!(matches!(
        to_proto(&Value::ZonedDatetime(zoned)).kind,
        Some(Kind::ZonedDatetime(_))
    ));

    // A datetime finer than Grafeo's microsecond is refused, not rounded.
    let mut fine = to_proto(&Value::Timestamp(timestamp));
    if let Some(Kind::LocalDatetime(dt)) = fine.kind.as_mut() {
        dt.time.get_or_insert_default().nanosecond += 1;
    }
    assert!(from_proto(&fine).is_err());
    // An impossible date is refused.
    let bad = proto(Kind::Date(Box::new(pb::Date {
        year: 2026,
        month: 2,
        day: 30,
        ..Default::default()
    })));
    assert!(from_proto(&bad).is_err());
}

#[test]
fn node_relationship_path_round_trip() {
    // What the engine itself answers, not a hand-built imitation of it.
    let engine = Engine::new();
    let graph = Graph::open(&engine, "ns", "values", OpenSpec::default()).expect("opens");
    graph
        .execute(
            "INSERT (:Person {name: 'Ada', born: 1815})-[:KNOWS {since: 1833}]->(:Person:Admin {name: 'Charles'})",
            false,
        )
        .expect("inserts");
    let result = graph
        .execute(
            "MATCH p = (a:Person {name: 'Ada'})-[r:KNOWS]->(b) RETURN a, r, b, p",
            true,
        )
        .expect("reads");
    assert_eq!(result.rows.len(), 1, "{result:?}");
    let row = &result.rows[0].values;

    let a = to_proto(&row[0]);
    let Some(Kind::Node(node)) = &a.kind else {
        panic!("a is not a Node: {:?}", row[0]);
    };
    assert_eq!(node.labels, vec!["Person".to_string()]);
    assert!(matches!(
        node.properties.get("born").and_then(|v| v.kind.as_ref()),
        Some(Kind::Int64(1815))
    ));
    assert!(!node.properties.contains_key("_id"));

    let r = to_proto(&row[1]);
    let Some(Kind::Relationship(rel)) = &r.kind else {
        panic!("r is not a Relationship: {:?}", row[1]);
    };
    assert_eq!(rel.r#type, "KNOWS");
    assert_eq!(rel.src, node.id);
    assert!(matches!(
        rel.properties.get("since").and_then(|v| v.kind.as_ref()),
        Some(Kind::Int64(1833))
    ));

    let b = to_proto(&row[2]);
    let Some(Kind::Node(other)) = &b.kind else {
        panic!("b is not a Node: {:?}", row[2]);
    };
    assert_eq!(rel.dst, other.id);
    let mut labels = other.labels.clone();
    labels.sort();
    assert_eq!(labels, vec!["Admin".to_string(), "Person".to_string()]);

    let p = to_proto(&row[3]);
    let Some(Kind::Path(path)) = &p.kind else {
        panic!("p is not a Path: {:?}", row[3]);
    };
    assert_eq!(path.nodes.len(), 2);
    assert_eq!(path.relationships.len(), 1);
    assert_eq!(path.nodes[0].id, node.id);
    assert_eq!(path.relationships[0].id, rel.id);

    for value in row {
        assert_eq!(&round_trip(value), value);
    }

    // A map that merely has an `_id` key is a map, not a node.
    let plain = map(&[("_id", Value::Int64(3)), ("x", s("y"))]);
    assert!(matches!(to_proto(&plain).kind, Some(Kind::Map(_))));
    assert_eq!(round_trip(&plain), plain);
}

#[test]
fn nested_list_map_round_trip() {
    let nested = list(vec![
        Value::Null,
        Value::Bool(true),
        Value::Float64(1.5),
        Value::Float64(f64::INFINITY),
        s("text"),
        Value::Bytes(Arc::from(vec![0_u8, 255, 7])),
        list(vec![]),
        map(&[]),
        map(&[
            (
                "inner",
                list(vec![Value::Int64(i64::MAX), map(&[("deep", s("x"))])]),
            ),
            (
                "when",
                Value::Date(Date::from_ymd(2000, 1, 1).expect("a date")),
            ),
        ]),
        Value::Vector(Arc::from(vec![0.25_f32, -1.0, 3.5])),
    ]);
    assert_eq!(round_trip(&nested), nested);

    let mut positive = HashMap::new();
    positive.insert("r1".to_string(), 3_u64);
    let mut negative = HashMap::new();
    negative.insert("r2".to_string(), 1_u64);
    let grow = Value::GCounter(Arc::new(positive.clone()));
    let on = Value::OnCounter {
        pos: Arc::new(positive),
        neg: Arc::new(negative),
    };
    assert_eq!(round_trip(&grow), grow);
    assert_eq!(round_trip(&on), on);

    // NaN is not equal to itself, so it is checked by kind.
    match round_trip(&Value::Float64(f64::NAN)) {
        Value::Float64(f) => assert!(f.is_nan()),
        other => panic!("{other:?}"),
    }
    // An empty `Value` is null; a decimal is refused (Grafeo has no decimal type).
    assert_eq!(
        from_proto(&pb::Value::default()).expect("null"),
        Value::Null
    );
    let decimal = proto(Kind::Decimal(Box::new(pb::Decimal {
        value: "1.10".into(),
        ..Default::default()
    })));
    assert!(from_proto(&decimal).is_err());
}

// ---------------------------------------------------------------------------------------------
// The JSON golden the desktop Graph page reads (Task 7)
// ---------------------------------------------------------------------------------------------

/// The cases of `conformance/graph/desktop/values.json`, by name.
fn golden_cases() -> Vec<(&'static str, Value)> {
    let node = map(&[
        ("_id", Value::Int64(1)),
        ("_labels", list(vec![s("Person")])),
        ("name", s("Ada")),
    ]);
    let edge = map(&[
        ("_id", Value::Int64(0)),
        ("_type", s("KNOWS")),
        ("_source", Value::Int64(1)),
        ("_target", Value::Int64(2)),
        ("since", Value::Int64(1833)),
    ]);
    vec![
        ("null", Value::Null),
        ("boolean", Value::Bool(true)),
        ("int64_max", Value::Int64(i64::MAX)),
        ("int64_2_53_plus_1", Value::Int64((1 << 53) + 1)),
        ("int64_negative", Value::Int64(-42)),
        ("float64", Value::Float64(-2.5)),
        ("float64_nan", Value::Float64(f64::NAN)),
        ("string", s("héllo")),
        ("bytes", Value::Bytes(Arc::from(vec![0_u8, 1, 254, 255]))),
        (
            "date",
            Value::Date(Date::from_ymd(2026, 10, 8).expect("date")),
        ),
        (
            "local_time",
            Value::Time(Time::from_hms_nano(13, 14, 15, 16).expect("time")),
        ),
        (
            "zoned_time",
            Value::Time(Time::from_hms(9, 0, 0).expect("time").with_offset(3600)),
        ),
        (
            "local_datetime",
            Value::Timestamp(Timestamp::from_micros(1_791_504_000_000_001)),
        ),
        (
            "zoned_datetime",
            Value::ZonedDatetime(ZonedDatetime::from_timestamp_offset(
                Timestamp::from_micros(1_791_504_000_000_000),
                -7 * 3600,
            )),
        ),
        (
            "duration",
            Value::Duration(Duration::new(1, 2, 3_000_000_000)),
        ),
        ("list", list(vec![Value::Int64(1), s("two"), Value::Null])),
        ("map", map(&[("a", Value::Int64(1)), ("b", list(vec![]))])),
        ("node", node),
        ("relationship", edge),
        (
            // As Grafeo 0.5.43 builds one: element ids.
            "path",
            Value::Path {
                nodes: Arc::from(vec![Value::Int64(1), Value::Int64(2)]),
                edges: Arc::from(vec![Value::Int64(7)]),
            },
        ),
        ("vector", Value::Vector(Arc::from(vec![0.5_f32, -0.25]))),
    ]
}

/// JSON with every object's keys sorted. A proto map is a hash map, so its serialization order is
/// not stable; the golden compares content, not order.
fn sorted(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(sorted).collect())
        }
        other => other,
    }
}

#[test]
fn connect_json_mapping_golden() {
    let path = repo_root().join("conformance/graph/desktop/values.json");
    let rendered: Vec<serde_json::Value> = golden_cases()
        .into_iter()
        .map(|(name, value)| {
            serde_json::json!({
                "name": name,
                "value": sorted(serde_json::to_value(to_proto(&value)).expect("serializes")),
            })
        })
        .collect();
    let document = serde_json::json!({
        "description": "The proto3 JSON (Connect JSON) form of loams.graph.v1.Value for every type Grafeo has (design §48 §8.2, GR1 Task 2). Generated by crates/loams-graph/tests/proto.rs `connect_json_mapping_golden`; regenerate with UPDATE_GOLDEN=1. The desktop Graph page (Task 8) and the mock (Task 7) read it.",
        "cases": rendered,
    });
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&document).expect("renders")
    );
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, &text).expect("writes the golden");
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{}: {err}; run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(
        text,
        committed,
        "{} is stale: run UPDATE_GOLDEN=1 cargo test -p loams-graph --test proto",
        path.display()
    );

    // And the other direction: every committed JSON form parses and decodes to the value it
    // was rendered from.
    let parsed: serde_json::Value = serde_json::from_str(&committed).expect("parses");
    let cases = parsed["cases"].as_array().expect("cases");
    for ((name, value), case) in golden_cases().iter().zip(cases) {
        assert_eq!(case["name"], *name);
        let wire: pb::Value = serde_json::from_value(case["value"].clone())
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        let decoded = from_proto(&wire).unwrap_or_else(|err| panic!("{name}: {err}"));
        match (value, &decoded) {
            (Value::Float64(a), Value::Float64(b)) if a.is_nan() => assert!(b.is_nan()),
            _ => assert_eq!(&decoded, value, "{name}"),
        }
    }
}
