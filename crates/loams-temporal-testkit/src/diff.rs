//! Field-by-field differences between two normalised histories.

use std::fmt;

use serde_json::Value;

/// One difference: where, and both values.
#[derive(Debug, Clone, PartialEq)]
pub struct Difference {
    /// The JSON path, starting at the history (`events[3].…`).
    pub path: String,
    /// The expected value (`null` when absent).
    pub expected: Value,
    /// The actual value (`null` when absent).
    pub actual: Value,
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: expected {} got {}",
            self.path, self.expected, self.actual
        )
    }
}

/// Every leaf where `expected` and `actual` differ (absent fields read as
/// `null`). Event lists of different lengths report the length and the
/// common prefix's differences.
pub fn diff(expected: &Value, actual: &Value) -> Vec<Difference> {
    let mut out = Vec::new();
    walk(String::new(), expected, actual, &mut out);
    out
}

fn walk(path: String, a: &Value, b: &Value, out: &mut Vec<Difference>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                walk(
                    p,
                    x.get(k).unwrap_or(&Value::Null),
                    y.get(k).unwrap_or(&Value::Null),
                    out,
                );
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push(Difference {
                    path: format!("{path}.length"),
                    expected: x.len().into(),
                    actual: y.len().into(),
                });
            }
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                walk(format!("{path}[{i}]"), p, q, out);
            }
        }
        _ if a == b => {}
        _ => out.push(Difference {
            path,
            expected: a.clone(),
            actual: b.clone(),
        }),
    }
}
