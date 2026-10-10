//! The desktop Graph page's contract fixtures, `conformance/graph/desktop/*.json` (design §48
//! §18.2, GR1 Task 7): what a fixture holds, and how an answer is compared with one.
//!
//! Three readers share them: the server's test (`crates/loams-graph/tests/desktop_contract.rs`),
//! this mock (which answers from them) and the page's tests (`web/plugins/graph`, GR1 Task 8).
//!
//! A fixture is `{ description, exchanges: [..] }`. An exchange is one call, in proto3 JSON
//! (Connect JSON):
//!
//! - `method`: `<package>.<Service>/<Method>`.
//! - `request`: the request message.
//! - one answer: `response` (a unary message), `chunks` (a server stream's messages, in order) or
//!   `error` (`{ code, message, reason, metadata }`: the Connect code in its wire spelling and the
//!   `loams.errors.v1.ErrorInfo` of the error).
//! - `ignore`: paths left out of the comparison because they differ per server or per run
//!   (`elapsedNanos`, ids, times). A path is keys joined by `.`; `key[]` is every element of the
//!   array at `key` (`[]` alone: of the answer itself); `**.key` is `key` at any depth.
//! - `contains`: the answer must hold the fixture's fields, and each array element the fixture
//!   lists, rather than equal it (for `GetInstance`, whose answer lists every package).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Every fixture, by file name, compiled in so the mock answers from them wherever it runs.
pub const FIXTURES: [(&str, &str); 9] = [
    (
        "instance.json",
        include_str!("../../../../conformance/graph/desktop/instance.json"),
    ),
    (
        "list_graphs.json",
        include_str!("../../../../conformance/graph/desktop/list_graphs.json"),
    ),
    (
        "schema.json",
        include_str!("../../../../conformance/graph/desktop/schema.json"),
    ),
    (
        "execute_table.json",
        include_str!("../../../../conformance/graph/desktop/execute_table.json"),
    ),
    (
        "execute_graph.json",
        include_str!("../../../../conformance/graph/desktop/execute_graph.json"),
    ),
    (
        "execute_truncated.json",
        include_str!("../../../../conformance/graph/desktop/execute_truncated.json"),
    ),
    (
        "explain.json",
        include_str!("../../../../conformance/graph/desktop/explain.json"),
    ),
    (
        "error_syntax.json",
        include_str!("../../../../conformance/graph/desktop/error_syntax.json"),
    ),
    (
        "error_denied.json",
        include_str!("../../../../conformance/graph/desktop/error_denied.json"),
    ),
];

/// The statements that build the `movies` graph the fixtures read, one per line (`--` lines are
/// comments). Run in order on an empty graph, they give the same ids every time.
pub const MOVIES_GQL: &str = include_str!("../../../../conformance/graph/desktop/movies.gql");

/// `values.json`: the proto3 JSON form of every `loams.graph.v1.Value` kind (GR1 Task 2).
pub const VALUES_JSON: &str = include_str!("../../../../conformance/graph/desktop/values.json");

/// The namespace the fixtures' graphs live in.
pub const NAMESPACE: &str = "default";

/// One fixture file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fixture {
    /// What the fixture pins, for a reader.
    pub description: String,
    /// The calls, in order.
    pub exchanges: Vec<Exchange>,
}

/// One call and its answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exchange {
    /// `<package>.<Service>/<Method>`.
    pub method: String,
    /// The request, in proto3 JSON.
    pub request: Value,
    /// Compare as "the answer holds this" rather than "the answer is this".
    #[serde(default, skip_serializing_if = "is_false")]
    pub contains: bool,
    /// Paths left out of the comparison.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
    /// A unary answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<Value>,
    /// A server stream's messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<Value>>,
    /// A failed call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorAnswer>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !value
}

/// A failed call: its Connect code and its `loams.errors.v1.ErrorInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorAnswer {
    /// The Connect code in its wire spelling, for example `invalid_argument`.
    pub code: String,
    /// The error's message.
    pub message: String,
    /// `ErrorInfo.reason`.
    pub reason: String,
    /// `ErrorInfo.metadata`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub metadata: std::collections::BTreeMap<String, String>,
}

/// What a call answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// A unary message.
    Response(Value),
    /// A server stream's messages, in order.
    Chunks(Vec<Value>),
    /// A failure.
    Error(ErrorAnswer),
}

impl Answer {
    /// The answer as one JSON value, the form the comparison works on.
    fn to_value(&self) -> Value {
        match self {
            Self::Response(value) => value.clone(),
            Self::Chunks(chunks) => Value::Array(chunks.clone()),
            Self::Error(error) => serde_json::to_value(error).unwrap_or(Value::Null),
        }
    }
}

impl Exchange {
    /// The fixture's answer; `None` when it has none yet (a fixture being written).
    #[must_use]
    pub fn answer(&self) -> Option<Answer> {
        if let Some(response) = &self.response {
            return Some(Answer::Response(response.clone()));
        }
        if let Some(chunks) = &self.chunks {
            return Some(Answer::Chunks(chunks.clone()));
        }
        self.error.clone().map(Answer::Error)
    }

    /// Whether `actual` matches the fixture's answer, outside the ignored paths.
    ///
    /// # Errors
    ///
    /// A message naming the method and showing both answers when they differ, or when the
    /// fixture has no answer.
    pub fn check(&self, actual: &Answer) -> Result<(), String> {
        let Some(expected) = self.answer() else {
            return Err(format!("{}: the fixture has no answer yet", self.method));
        };
        let (mut want, mut got) = (expected.to_value(), actual.to_value());
        for path in &self.ignore {
            let path: Vec<&str> = path.split('.').collect();
            strip(&mut want, &path);
            strip(&mut got, &path);
        }
        let matches = if self.contains {
            holds(&got, &want)
        } else {
            got == want
        };
        if matches {
            Ok(())
        } else {
            Err(format!(
                "{} {}\nexpected{}: {}\nactual: {}",
                self.method,
                self.request,
                if self.contains { " (contained)" } else { "" },
                pretty(&sorted(want)),
                pretty(&sorted(got)),
            ))
        }
    }

    /// Replaces the fixture's answer with `actual`, keeping the old answer's values at the
    /// ignored paths so that regenerating a fixture does not churn its ids and times. Map keys are
    /// sorted, so the file is stable.
    pub fn record(&mut self, actual: Answer) {
        let old = self.answer().map(|answer| answer.to_value());
        let mut new = actual.to_value();
        if let Some(old) = &old {
            for path in &self.ignore {
                let path: Vec<&str> = path.split('.').collect();
                carry(&mut new, old, &path);
            }
        }
        let new = sorted(new);
        self.response = None;
        self.chunks = None;
        self.error = None;
        match actual {
            Answer::Response(_) => self.response = Some(new),
            Answer::Chunks(_) => {
                self.chunks = Some(match new {
                    Value::Array(items) => items,
                    other => vec![other],
                });
            }
            Answer::Error(_) => self.error = serde_json::from_value(new).ok(),
        }
    }
}

/// Every fixture, parsed, by file name.
///
/// # Panics
///
/// When a compiled-in fixture is not valid: the files are part of this build.
#[must_use]
pub fn fixtures() -> Vec<(&'static str, Fixture)> {
    FIXTURES
        .iter()
        .map(|(name, text)| {
            let fixture = serde_json::from_str(text)
                .unwrap_or_else(|err| panic!("conformance/graph/desktop/{name}: {err}"));
            (*name, fixture)
        })
        .collect()
}

/// The statements of [`MOVIES_GQL`], in order.
pub fn movies_statements() -> impl Iterator<Item = &'static str> {
    MOVIES_GQL
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("--"))
}

/// A fixture as its file holds it: pretty JSON and a final newline.
#[must_use]
pub fn render(fixture: &Fixture) -> String {
    let mut text = serde_json::to_string_pretty(fixture).unwrap_or_default();
    text.push('\n');
    text
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// `value` with every object's keys sorted (proto3 JSON maps come out in hash order).
#[must_use]
pub fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// Splits an ignore path's first segment into its key and whether it means every element.
fn segment(head: &str) -> (&str, bool) {
    head.strip_suffix("[]")
        .map_or((head, false), |key| (key, true))
}

/// Removes what `path` names from `value`.
fn strip(value: &mut Value, path: &[&str]) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    if *head == "**" {
        strip(value, rest);
        match value {
            Value::Object(map) => map.values_mut().for_each(|v| strip(v, path)),
            Value::Array(items) => items.iter_mut().for_each(|v| strip(v, path)),
            _ => {}
        }
        return;
    }
    let (key, each) = segment(head);
    if rest.is_empty() && !each {
        if let Value::Object(map) = value {
            map.remove(key);
        }
        return;
    }
    let inner = if key.is_empty() {
        Some(value)
    } else {
        value.get_mut(key)
    };
    match inner {
        Some(Value::Array(items)) if each => items.iter_mut().for_each(|v| strip(v, rest)),
        Some(inner) if !each => strip(inner, rest),
        _ => {}
    }
}

/// Copies what `path` names from `old` into `new`, where both have it.
fn carry(new: &mut Value, old: &Value, path: &[&str]) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    if *head == "**" {
        carry(new, old, rest);
        match (new, old) {
            (Value::Object(n), Value::Object(o)) => {
                for (key, value) in n.iter_mut() {
                    if let Some(old) = o.get(key) {
                        carry(value, old, path);
                    }
                }
            }
            (Value::Array(n), Value::Array(o)) => {
                n.iter_mut().zip(o).for_each(|(n, o)| carry(n, o, path));
            }
            _ => {}
        }
        return;
    }
    let (key, each) = segment(head);
    if rest.is_empty() && !each {
        if let (Value::Object(map), Some(old)) = (new, old.get(key)) {
            map.insert(key.to_string(), old.clone());
        }
        return;
    }
    let (inner, old) = if key.is_empty() {
        (Some(new), Some(old))
    } else {
        (new.get_mut(key), old.get(key))
    };
    match (inner, old) {
        (Some(Value::Array(n)), Some(Value::Array(o))) if each => {
            n.iter_mut().zip(o).for_each(|(n, o)| carry(n, o, rest));
        }
        (Some(inner), Some(old)) if !each => carry(inner, old, rest),
        _ => {}
    }
}

/// Whether `actual` holds everything `expected` has: an object's fields (recursively), each of an
/// array's elements somewhere in the actual array, and any other value exactly.
fn holds(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => e
            .iter()
            .all(|(key, value)| a.get(key).is_some_and(|got| holds(got, value))),
        (Value::Array(a), Value::Array(e)) => {
            e.iter().all(|want| a.iter().any(|got| holds(got, want)))
        }
        _ => actual == expected,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn exchange(ignore: &[&str], response: Value) -> Exchange {
        Exchange {
            method: "m".into(),
            request: json!({}),
            contains: false,
            ignore: ignore.iter().map(|s| (*s).to_string()).collect(),
            response: Some(response),
            chunks: None,
            error: None,
        }
    }

    #[test]
    fn ignored_paths_are_not_compared() {
        let fixture = exchange(
            &["graphs[].id", "**.elapsedNanos", "[].x"],
            json!({"graphs": [{"id": "a", "name": "kg"}], "root": {"elapsedNanos": "1", "children": [{"elapsedNanos": "2"}]}}),
        );
        let actual = json!({"graphs": [{"id": "b", "name": "kg"}], "root": {"elapsedNanos": "9", "children": [{"elapsedNanos": "8"}]}});
        assert_eq!(fixture.check(&Answer::Response(actual)), Ok(()));
        let other = json!({"graphs": [{"id": "b", "name": "movies"}], "root": {"children": [{}]}});
        assert!(fixture.check(&Answer::Response(other)).is_err());
    }

    #[test]
    fn contains_matches_a_subset() {
        let mut fixture = exchange(
            &[],
            json!({"services": [{"package": "g", "available": true}]}),
        );
        fixture.contains = true;
        let actual = json!({"name": "x", "services": [{"package": "a"}, {"package": "g", "available": true, "version": "v1"}]});
        assert_eq!(fixture.check(&Answer::Response(actual)), Ok(()));
        let unavailable = json!({"services": [{"package": "g"}]});
        assert!(fixture.check(&Answer::Response(unavailable)).is_err());
    }

    #[test]
    fn record_keeps_ignored_values() {
        let mut fixture = exchange(
            &["graphs[].id"],
            json!({"graphs": [{"id": "kept", "name": "kg"}]}),
        );
        fixture.record(Answer::Response(
            json!({"graphs": [{"name": "kg", "id": "new"}]}),
        ));
        assert_eq!(
            fixture.response,
            Some(json!({"graphs": [{"id": "kept", "name": "kg"}]}))
        );
    }

    #[test]
    fn every_fixture_parses_and_has_an_answer() {
        let fixtures = fixtures();
        assert_eq!(fixtures.len(), FIXTURES.len());
        for (name, fixture) in fixtures {
            assert!(!fixture.exchanges.is_empty(), "{name}");
            for exchange in &fixture.exchanges {
                assert!(exchange.answer().is_some(), "{name}: {}", exchange.method);
            }
        }
        assert!(movies_statements().count() > 0);
    }
}
