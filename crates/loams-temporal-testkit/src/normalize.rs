//! The corpus's normalisation (`conformance/compute/temporal/corpus/
//! normalize.py`), ported key by key, so two histories of the same scenario
//! compare equal exactly when they differ only in what the server or worker
//! picks at run time.
//!
//! Masked: event times and every other timestamp (keys ending in `Time`,
//! `Timestamp` or `_time`, and RFC 3339 times inside strings); run IDs (keys
//! ending in `RunId`, and UUIDs inside strings); identities; task tokens;
//! request IDs; binary checksums and build IDs; the namespace and its ID;
//! task IDs; `historySizeBytes`; stack traces; a continued run's
//! `firstWorkflowTaskBackoff`; sticky queue names; times and UUIDs inside
//! payloads. SDK flag sets are sorted. Everything else is kept.

use loams_temporal_proto::temporal::api::history::v1::History;
use serde_json::{Map, Value};

/// Scenarios whose parallel activities start and complete in an order the
/// server picks; they compare with that order free.
pub const ORDER_FREE: &[&str] = &["activity_parallel_fanout_10"];

/// Extra equivalences beyond `normalize.py`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// A sticky task queue reads as the normal queue it stands for (the
    /// gateway has no sticky queues in DX1, Q1156; the SDKs do not replay
    /// task queues).
    pub sticky_as_normal: bool,
    /// Compare the activity started and completed events as a multiset
    /// (`ORDER_FREE` scenarios).
    pub activity_order_free: bool,
}

impl Options {
    /// The options for comparing a gateway history with the corpus's
    /// history of `scenario`.
    pub fn gateway(scenario: &str) -> Options {
        Options {
            sticky_as_normal: true,
            activity_order_free: ORDER_FREE.contains(&scenario),
        }
    }
}

fn mask_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "identity" => "<identity>",
        "requestId" => "<request-id>",
        "taskToken" => "<task-token>",
        "binaryChecksum" => "<binary-checksum>",
        "buildId" => "<build-id>",
        "namespace" | "parentWorkflowNamespace" => "<namespace>",
        "namespaceId" | "parentWorkflowNamespaceId" => "<namespace-id>",
        "taskId" => "<task-id>",
        "historySizeBytes" => "<size>",
        "stackTrace" => "<stack-trace>",
        "firstWorkflowTaskBackoff" => "<backoff>",
        _ => return None,
    })
}

fn is_time_key(key: &str) -> bool {
    key.ends_with("Time") || key.ends_with("Timestamp") || key.ends_with("_time")
}

fn is_hex(b: &[u8]) -> bool {
    b.iter().all(u8::is_ascii_hexdigit)
}

/// Replaces UUIDs with `<uuid>` and RFC 3339 times with `<time>`.
fn mask_text(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if let Some(n) = uuid_at(b, i) {
            out.push_str("<uuid>");
            i += n;
        } else if let Some(n) = rfc3339_at(b, i) {
            out.push_str("<time>");
            i += n;
        } else {
            let ch = s[i..].chars().next().expect("in bounds");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn uuid_at(b: &[u8], i: usize) -> Option<usize> {
    let parts = [8, 4, 4, 4, 12];
    let mut j = i;
    for (k, len) in parts.iter().enumerate() {
        let end = j + len;
        if end > b.len() || !is_hex(&b[j..end]) {
            return None;
        }
        j = end;
        if k < 4 {
            if b.get(j) != Some(&b'-') {
                return None;
            }
            j += 1;
        }
    }
    Some(j - i)
}

fn digits(b: &[u8], i: usize, n: usize) -> bool {
    i + n <= b.len() && b[i..i + n].iter().all(u8::is_ascii_digit)
}

fn rfc3339_at(b: &[u8], i: usize) -> Option<usize> {
    // \d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})
    let lit = |j: usize, c: u8| b.get(j) == Some(&c);
    if !(digits(b, i, 4)
        && lit(i + 4, b'-')
        && digits(b, i + 5, 2)
        && lit(i + 7, b'-')
        && digits(b, i + 8, 2)
        && lit(i + 10, b'T')
        && digits(b, i + 11, 2)
        && lit(i + 13, b':')
        && digits(b, i + 14, 2)
        && lit(i + 16, b':')
        && digits(b, i + 17, 2))
    {
        return None;
    }
    let mut j = i + 19;
    if lit(j, b'.') && digits(b, j + 1, 1) {
        j += 1;
        while digits(b, j, 1) {
            j += 1;
        }
    }
    if lit(j, b'Z') {
        return Some(j + 1 - i);
    }
    if (lit(j, b'+') || lit(j, b'-'))
        && digits(b, j + 1, 2)
        && lit(j + 3, b':')
        && digits(b, j + 4, 2)
    {
        return Some(j + 6 - i);
    }
    None
}

fn b64_text(v: &str) -> Option<String> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD.decode(v).ok()?;
    String::from_utf8(raw).ok()
}

fn sort_keys(v: &Value) -> String {
    // serde_json's Map is a BTreeMap without `preserve_order`: keys sorted.
    serde_json::to_string(v).expect("JSON values serialize")
}

fn mask(key: Option<&str>, v: Value, opts: &Options) -> Value {
    if let Some(k) = key
        && is_time_key(k)
        && (v.is_string() || v.is_object())
    {
        return Value::String("<time>".into());
    }
    match v {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, x) in map {
                let masked = mask(Some(&k), x, opts);
                out.insert(k, masked);
            }
            if out.get("kind") == Some(&Value::String("TASK_QUEUE_KIND_STICKY".into()))
                && out.contains_key("name")
            {
                if opts.sticky_as_normal {
                    let normal = out.remove("normalName").unwrap_or(Value::Null);
                    out.insert("name".into(), normal);
                    out.insert(
                        "kind".into(),
                        Value::String("TASK_QUEUE_KIND_NORMAL".into()),
                    );
                } else {
                    out.insert("name".into(), Value::String("<sticky-queue>".into()));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            let mut out: Vec<Value> = items.into_iter().map(|x| mask(key, x, opts)).collect();
            if matches!(key, Some("langUsedFlags" | "coreUsedFlags")) {
                out.sort_by_key(sort_keys);
            }
            Value::Array(out)
        }
        Value::String(s) => {
            if let Some(k) = key {
                if let Some(m) = mask_key(k) {
                    return Value::String(m.into());
                }
                if k.ends_with("RunId") || k == "runId" {
                    return Value::String("<run-id>".into());
                }
                if k == "data"
                    && let Some(text) = b64_text(&s)
                {
                    let mut masked = mask_text(&text);
                    if let Ok(doc) = serde_json::from_str::<Value>(&text) {
                        let remasked = sort_keys(&mask(None, doc.clone(), opts));
                        if remasked != sort_keys(&doc) {
                            masked = remasked;
                        }
                    }
                    return if masked == text {
                        Value::String(s)
                    } else {
                        Value::String(format!("<decoded> {masked}"))
                    };
                }
            }
            Value::String(mask_text(&s))
        }
        other => {
            if let Some(k) = key
                && let Some(m) = mask_key(k)
            {
                return Value::String(m.into());
            }
            other
        }
    }
}

fn without_ids(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| *k != "eventId" && *k != "startedEventId")
                .map(|(k, x)| (k.clone(), without_ids(x)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(without_ids).collect()),
        x => x.clone(),
    }
}

/// `history` (protobuf JSON with an `events` list) with run-time choices
/// masked.
pub fn normalize(history: Value, opts: &Options) -> Value {
    let mut h = mask(None, history, opts);
    if opts.activity_order_free {
        let done = [
            "EVENT_TYPE_ACTIVITY_TASK_STARTED",
            "EVENT_TYPE_ACTIVITY_TASK_COMPLETED",
        ];
        let events: Vec<Value> = h["events"].as_array().cloned().unwrap_or_default();
        let is_done = |e: &Value| done.contains(&e["eventType"].as_str().unwrap_or(""));
        let mut activity: Vec<String> = events
            .iter()
            .filter(|e| is_done(e))
            .map(|e| sort_keys(&without_ids(e)))
            .collect();
        activity.sort();
        let types: Vec<Value> = events.iter().map(|e| e["eventType"].clone()).collect();
        let rest: Vec<Value> = events.into_iter().filter(|e| !is_done(e)).collect();
        h["events"] = Value::Array(rest);
        h["activityEvents"] = Value::Array(activity.into_iter().map(Value::String).collect());
        h["eventTypes"] = Value::Array(types);
    }
    h
}

/// [`normalize`] of a decoded `History`.
pub fn normalized_history(history: &History, opts: &Options) -> Value {
    normalize(crate::json::history_to_json(history), opts)
}

/// The differences between two normalised histories, as readable lines
/// (empty when equal), like `normalize.py --compare`.
pub fn compare(a: &Value, b: &Value) -> Vec<String> {
    crate::diff::diff(a, b)
        .into_iter()
        .map(|d| d.to_string())
        .collect()
}
