//! Aggregations (#951, pulled forward from design 06's Phase B): a
//! search body's `aggs` (or `aggregations`) runs on the engine's
//! aggregations (Tantivy's, in Elasticsearch's request and result shape),
//! over one index.
//!
//! The gateway checks the tree (the aggregation types the engine serves,
//! the fields against the mapping), answers ES's empty result for a
//! top-level bucket or metric aggregation on an unmapped field, as ES does,
//! and renders `top_hits` as ES does (`hits.total`, `max_score`,
//! `_index`).

use serde_json::{Map, Value, json};

use crate::dsl::value::{FieldRef, es_type, resolve};
use crate::error::EsError;
use crate::mapping::IndexView;

/// The aggregation types the engine serves.
const TYPES: &[&str] = &[
    "terms",
    "composite",
    "top_hits",
    "filter",
    "range",
    "histogram",
    "date_histogram",
    "min",
    "max",
    "avg",
    "sum",
    "stats",
    "extended_stats",
    "value_count",
    "cardinality",
    "percentiles",
];

/// The keys a node holds besides its type.
const NODE_KEYS: &[&str] = &["aggs", "aggregations", "meta"];

/// `body` without `aggs`/`aggregations`, and those (`None` when absent).
pub(crate) fn split(body: &Value) -> Result<(Value, Option<Map<String, Value>>), EsError> {
    let Value::Object(map) = body else {
        return Ok((body.clone(), None));
    };
    let mut rest = map.clone();
    let aggs = rest.remove("aggs");
    let aggregations = rest.remove("aggregations");
    let aggs = match (aggs, aggregations) {
        (Some(_), Some(_)) => {
            return Err(EsError::parsing(
                "Found two aggregation type definitions in [aggs] and [aggregations]",
            ));
        }
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => return Ok((Value::Object(rest), None)),
    };
    match aggs {
        Value::Object(aggs) => Ok((Value::Object(rest), Some(aggs))),
        Value::Null => Ok((Value::Object(rest), None)),
        _ => Err(EsError::parsing(
            "Expected [START_OBJECT] under [aggs], but got a value",
        )),
    }
}

/// The engine's request, and the results the gateway answers itself (a
/// top-level aggregation on an unmapped field).
pub(crate) struct Plan {
    pub engine: Map<String, Value>,
    pub synthesized: Map<String, Value>,
}

/// The node's type and its body.
fn node_type<'a>(
    name: &str,
    node: &'a Map<String, Value>,
) -> Result<(&'a str, &'a Value), EsError> {
    let mut found = None;
    for (key, value) in node {
        if NODE_KEYS.contains(&key.as_str()) {
            continue;
        }
        if found.is_some() {
            return Err(EsError::parsing(format!(
                "Found two aggregation type definitions in [{name}]: [{}] and [{key}]",
                found.map(|(k, _)| k).unwrap_or_default()
            )));
        }
        found = Some((key.as_str(), value));
    }
    found.ok_or_else(|| EsError::parsing(format!("Missing definition for aggregation [{name}]")))
}

/// The fields a node's body names: `field`, and a composite's sources'.
fn fields_of(kind: &str, body: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(field) = body.get("field").and_then(Value::as_str) {
        out.push(field.to_string());
    }
    if kind == "composite"
        && let Some(sources) = body.get("sources").and_then(Value::as_array)
    {
        for source in sources {
            for (_, spec) in source.as_object().into_iter().flatten() {
                for (_, inner) in spec.as_object().into_iter().flatten() {
                    if let Some(field) = inner.get("field").and_then(Value::as_str) {
                        out.push(field.to_string());
                    }
                }
            }
        }
    }
    out
}

fn fielddata(field: &str) -> EsError {
    EsError::illegal_argument(format!(
        "Fielddata is disabled on [{field}] in [_aggs]. Text fields are not optimised for \
         operations that require per-document field data like aggregations and sorting, so \
         these operations are disabled by default. Please use a keyword field instead. \
         Alternatively, set fielddata=true on [{field}] in order to load field data by \
         uninverting the inverted index. Note that this can use significant memory."
    ))
}

/// ES's answer for a top-level aggregation over an unmapped field.
fn empty_result(kind: &str) -> Option<Value> {
    Some(match kind {
        "terms" => {
            json!({"doc_count_error_upper_bound": 0, "sum_other_doc_count": 0, "buckets": []})
        }
        "composite" | "histogram" | "date_histogram" | "range" => json!({"buckets": []}),
        "min" | "max" | "avg" | "sum" => json!({"value": null}),
        "value_count" | "cardinality" => json!({"value": 0}),
        _ => return None,
    })
}

/// Checks `aggs` against `view` (rule: one type per node, a type the
/// engine serves, mapped non-text fields).
pub(crate) fn plan(view: &IndexView, aggs: &Map<String, Value>) -> Result<Plan, EsError> {
    let mut out = Plan {
        engine: Map::new(),
        synthesized: Map::new(),
    };
    for (name, node) in aggs {
        let Value::Object(node_map) = node else {
            return Err(EsError::parsing(format!(
                "Expected [START_OBJECT] under [{name}], but got a value"
            )));
        };
        let (kind, _) = node_type(name, node_map)?;
        if check_node(view, name, node_map, true)? {
            let mut node = node.clone();
            default_top_hits_sort(&mut node);
            out.engine.insert(name.clone(), node);
        } else {
            let empty = empty_result(kind).ok_or_else(|| {
                EsError::unsupported(&format!("[{kind}] aggregation on an unmapped field"))
            })?;
            out.synthesized.insert(name.clone(), empty);
        }
    }
    Ok(out)
}

/// Gives every `top_hits` without a `sort` an empty one: the engine needs
/// the key, and with no sort keys it returns the bucket's documents in
/// index order, where ES orders them by score (equal under a filter).
fn default_top_hits_sort(node: &mut Value) {
    let Value::Object(map) = node else {
        return;
    };
    if let Some(Value::Object(top)) = map.get_mut("top_hits") {
        top.entry("sort").or_insert_with(|| json!([]));
    }
    for key in ["aggs", "aggregations"] {
        if let Some(Value::Object(sub)) = map.get_mut(key) {
            for child in sub.values_mut() {
                default_top_hits_sort(child);
            }
        }
    }
}

/// `Ok(false)` when a top-level node names an unmapped field (its result
/// is ES's empty one).
fn check_node(
    view: &IndexView,
    name: &str,
    node: &Map<String, Value>,
    top: bool,
) -> Result<bool, EsError> {
    let (kind, body) = node_type(name, node)?;
    if !TYPES.contains(&kind) {
        return Err(EsError::unsupported(&format!("[{kind}] aggregation")));
    }
    let mut mapped = true;
    for field in fields_of(kind, body) {
        match resolve(view, &field) {
            FieldRef::Field(spec)
                if matches!(spec.kind, loams_collection::FieldKind::Text { .. })
                    || es_type(view, &field) == "text" =>
            {
                return Err(fielddata(&field));
            }
            FieldRef::Field(_) | FieldRef::Json { .. } => {}
            FieldRef::Unmapped if top => mapped = false,
            FieldRef::Unmapped => {
                return Err(EsError::unsupported(&format!(
                    "a sub-aggregation on the unmapped field [{field}]"
                )));
            }
            FieldRef::Id => return Err(EsError::unsupported("aggregations on _id")),
            FieldRef::Vector(_) => {
                return Err(EsError::unsupported("aggregations on a dense_vector field"));
            }
        }
    }
    let sub = node.get("aggs").or_else(|| node.get("aggregations"));
    if let Some(sub) = sub {
        let Value::Object(sub) = sub else {
            return Err(EsError::parsing(format!(
                "Expected [START_OBJECT] under [aggs] of [{name}], but got a value"
            )));
        };
        if kind == "top_hits" {
            return Err(EsError::parsing(format!(
                "Aggregator [{name}] of type [top_hits] cannot accept sub-aggregations"
            )));
        }
        for (sub_name, sub_node) in sub {
            let Value::Object(sub_node) = sub_node else {
                return Err(EsError::parsing(format!(
                    "Expected [START_OBJECT] under [{sub_name}], but got a value"
                )));
            };
            check_node(view, sub_name, sub_node, false)?;
        }
    }
    Ok(mapped)
}

/// The `aggregations` of the answer: the engine's results with `top_hits`
/// in ES's shape, and the synthesized ones, in the request's order.
pub(crate) fn render(
    request: &Map<String, Value>,
    engine: Option<Value>,
    mut synthesized: Map<String, Value>,
    index: &str,
) -> Value {
    let mut engine = match engine {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let mut out = Map::new();
    for (name, node) in request {
        if let Some(result) = synthesized.remove(name) {
            out.insert(name.clone(), result);
        } else if let Some(mut result) = engine.remove(name) {
            if let Value::Object(node) = node {
                fix(node, &mut result, None, index);
            }
            out.insert(name.clone(), result);
        }
    }
    Value::Object(out)
}

/// Walks one result with its request node; `doc_count` is the enclosing
/// bucket's.
fn fix(node: &Map<String, Value>, result: &mut Value, doc_count: Option<u64>, index: &str) {
    if node.contains_key("top_hits") {
        top_hits(result, doc_count, index);
        return;
    }
    let Some(sub) = node
        .get("aggs")
        .or_else(|| node.get("aggregations"))
        .and_then(Value::as_object)
    else {
        return;
    };
    let mut visit = |bucket: &mut Value| {
        let count = bucket.get("doc_count").and_then(Value::as_u64);
        for (name, sub_node) in sub {
            if let (Some(sub_node), Some(sub_result)) = (sub_node.as_object(), bucket.get_mut(name))
            {
                fix(sub_node, sub_result, count, index);
            }
        }
    };
    match result.get_mut("buckets") {
        Some(Value::Array(buckets)) => buckets.iter_mut().for_each(&mut visit),
        Some(Value::Object(buckets)) => buckets.values_mut().for_each(&mut visit),
        _ => visit(result),
    }
}

/// `{"hits": [...]}` as ES's `{"hits": {"total", "max_score", "hits"}}`,
/// each hit with `_index`.
fn top_hits(result: &mut Value, doc_count: Option<u64>, index: &str) {
    let hits = match result.get_mut("hits") {
        Some(Value::Array(hits)) => std::mem::take(hits),
        Some(Value::Object(_)) => return,
        _ => Vec::new(),
    };
    let hits: Vec<Value> = hits
        .into_iter()
        .map(|hit| {
            let mut out = Map::new();
            out.insert("_index".to_string(), json!(index));
            if let Value::Object(map) = hit {
                out.extend(map);
            }
            Value::Object(out)
        })
        .collect();
    let total = doc_count.unwrap_or(hits.len() as u64);
    *result = json!({
        "hits": {
            "total": {"value": total, "relation": "eq"},
            "max_score": null,
            "hits": hits,
        }
    });
}
