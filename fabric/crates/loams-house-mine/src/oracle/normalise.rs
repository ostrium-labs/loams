//! The normaliser and the comparison (HE0 Task 6).
//!
//! What the normaliser hides, and nothing else:
//!
//! - **Float formatting**: `Float32` and `Float64` values are rounded to a
//!   declared number of significant digits, so `0.30000000000000004` and `0.3`
//!   compare equal; `-0` is `0`; `nan`, `inf` and `-inf` keep their meaning
//!   whether they arrive as numbers or strings.
//! - **Row order**, when the case has no `ORDER BY`: rows compare as multisets.
//! - **Output-format quirks**: a 64-bit integer quoted (`"1"`) or not (`1`); a
//!   decimal with or without trailing zeros.
//!
//! What it never hides (Review Focus 3): the number of rows, the column names,
//! the column types (compared verbatim), the error code, and a duplicated or
//! missing row in a multiset. Each of these is its own [`Kind`], so triaging one
//! (say a type difference as an incompatibility) never covers another.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{Shape, shape};

/// One target's answer to one case.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// `JSONCompactEachRowWithNamesAndTypes`: names, types, rows.
    Rows(ResultSet),
    /// A ClickHouse exception: its code and the first line of its text.
    Error { code: u32, message: String },
}

/// A result set as the format writes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultSet {
    pub names: Vec<String>,
    pub types: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

/// Whether a case's rows have a defined order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Ordered,
    Unordered,
}

/// The declared float precision, in significant decimal digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Precision {
    pub float64: usize,
    pub float32: usize,
}

impl Default for Precision {
    /// 12 digits for `Float64` (summation order changes the last few) and 6 for
    /// `Float32` (about 7 are significant).
    fn default() -> Self {
        Self {
            float64: 12,
            float32: 6,
        }
    }
}

impl Precision {
    /// The same number of digits for both widths (a case's `precision = N`).
    pub fn uniform(digits: usize) -> Self {
        Self {
            float64: digits,
            float32: digits,
        }
    }
}

/// What differs between the baseline's outcome and a target's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// One side raised an exception and the other did not, or the codes differ.
    Error,
    /// The column names differ.
    Names,
    /// The column types differ (verbatim).
    Types,
    /// The number of rows differs.
    RowCount,
    /// Same number of rows, different values (or, unordered, different
    /// multiplicities).
    Values,
    /// A case in a named output format (`-- oracle: format = <F>`): the
    /// answers' bytes differ.
    Bytes,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Names => "names",
            Self::Types => "types",
            Self::RowCount => "row_count",
            Self::Values => "values",
            Self::Bytes => "bytes",
        }
    }
}

/// One difference, with a short description for the triage file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub kind: Kind,
    pub detail: String,
}

/// A value in canonical form. Ordered so rows can be sorted into multisets.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Canon {
    Null,
    Bool(bool),
    Int(String),
    Float(String),
    Decimal(String),
    Str(String),
    List(Vec<Canon>),
    Map(Vec<(String, Canon)>),
    /// A JSON value of a type the normaliser does not read, as written.
    Json(String),
}

/// Puts one value of a column of type `type_name` in canonical form.
pub fn canon(value: &Value, type_name: &str, precision: Precision) -> Canon {
    canon_shaped(value, &shape(type_name), precision)
}

fn canon_shaped(value: &Value, shape: &Shape, precision: Precision) -> Canon {
    match (shape, value) {
        (_, Value::Null) => Canon::Null,
        (Shape::Int, Value::Number(n)) => Canon::Int(n.to_string()),
        (Shape::Int, Value::String(s)) => Canon::Int(int_text(s)),
        (Shape::Float64, v) => float(v, precision.float64),
        (Shape::Float32, v) => float(v, precision.float32),
        (Shape::Decimal, Value::Number(n)) => Canon::Decimal(decimal_text(&n.to_string())),
        (Shape::Decimal, Value::String(s)) => Canon::Decimal(decimal_text(s)),
        (Shape::Array(element), Value::Array(items)) => Canon::List(
            items
                .iter()
                .map(|item| canon_shaped(item, element, precision))
                .collect(),
        ),
        (Shape::Tuple(elements), Value::Array(items)) if elements.len() == items.len() => {
            Canon::List(
                items
                    .iter()
                    .zip(elements)
                    .map(|(item, element)| canon_shaped(item, element, precision))
                    .collect(),
            )
        }
        (Shape::Tuple(elements), Value::Object(fields)) if elements.len() == fields.len() => {
            Canon::Map(
                fields
                    .iter()
                    .zip(elements)
                    .map(|((k, v), element)| (k.clone(), canon_shaped(v, element, precision)))
                    .collect(),
            )
        }
        (Shape::Map(element), Value::Object(fields)) => Canon::Map(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), canon_shaped(v, element, precision)))
                .collect(),
        ),
        (_, Value::Bool(b)) => Canon::Bool(*b),
        (_, Value::String(s)) => Canon::Str(s.clone()),
        (_, other) => Canon::Json(other.to_string()),
    }
}

/// `"0012"` → `12`; text that is not an integer stays as it is.
fn int_text(s: &str) -> String {
    let t = s.trim();
    if let Ok(i) = t.parse::<i128>() {
        i.to_string()
    } else if let Ok(u) = t.parse::<u128>() {
        u.to_string()
    } else {
        t.to_string()
    }
}

/// `1.2500` → `1.25`, `3.000` → `3`, `-0.0` → `0`.
fn decimal_text(s: &str) -> String {
    let t = s.trim();
    let t = if t.contains('.') {
        t.trim_end_matches('0').trim_end_matches('.')
    } else {
        t
    };
    match t {
        "-0" | "" => "0".to_string(),
        t => t.to_string(),
    }
}

fn float(value: &Value, digits: usize) -> Canon {
    let parsed = match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "nan" | "-nan" | "+nan" => Some(f64::NAN),
            "inf" | "+inf" | "infinity" => Some(f64::INFINITY),
            "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
            other => other.parse::<f64>().ok(),
        },
        _ => None,
    };
    match parsed {
        Some(f) if f.is_nan() => Canon::Float("nan".into()),
        Some(f) if f.is_infinite() => Canon::Float(if f > 0.0 { "inf" } else { "-inf" }.into()),
        Some(0.0) => Canon::Float("0".into()),
        Some(f) => {
            // Rounded to the declared digits, then written shortest, so the
            // canonical text does not depend on how many digits were kept.
            let rounded = format!("{:.*e}", digits.max(1) - 1, f);
            Canon::Float(rounded.parse::<f64>().map_or(rounded, |r| r.to_string()))
        }
        None => Canon::Json(value.to_string()),
    }
}

/// One row in canonical form, each value read with its own column's type.
pub fn canon_row(row: &[Value], types: &[String], precision: Precision) -> Vec<Canon> {
    row.iter()
        .enumerate()
        .map(|(i, v)| canon(v, types.get(i).map_or("", String::as_str), precision))
        .collect()
}

/// Compares a target's outcome with the baseline's. Every difference is
/// reported, each as its own [`Kind`]; an empty list is a match.
pub fn compare(
    baseline: &Outcome,
    other: &Outcome,
    order: Order,
    precision: Precision,
) -> Vec<Mismatch> {
    let (base, them) = match (baseline, other) {
        (Outcome::Rows(b), Outcome::Rows(o)) => (b, o),
        (Outcome::Error { code: a, .. }, Outcome::Error { code: b, .. }) if a == b => {
            return Vec::new();
        }
        (a, b) => {
            return vec![Mismatch {
                kind: Kind::Error,
                detail: format!("baseline {}, target {}", describe(a), describe(b)),
            }];
        }
    };
    let mut found = Vec::new();
    if base.names != them.names {
        found.push(Mismatch {
            kind: Kind::Names,
            detail: format!("{:?} vs {:?}", base.names, them.names),
        });
    }
    if base.types != them.types {
        found.push(Mismatch {
            kind: Kind::Types,
            detail: format!("{:?} vs {:?}", base.types, them.types),
        });
    }
    if base.rows.len() != them.rows.len() {
        found.push(Mismatch {
            kind: Kind::RowCount,
            detail: format!("{} rows vs {} rows", base.rows.len(), them.rows.len()),
        });
        // Values of unequal row counts are not compared: the row count is the
        // difference, and it has its own entry.
        return found;
    }
    let mut ours: Vec<Vec<Canon>> = base
        .rows
        .iter()
        .map(|r| canon_row(r, &base.types, precision))
        .collect();
    let mut theirs: Vec<Vec<Canon>> = them
        .rows
        .iter()
        .map(|r| canon_row(r, &them.types, precision))
        .collect();
    match order {
        Order::Ordered => {
            if let Some(i) = (0..ours.len()).find(|&i| ours[i] != theirs[i]) {
                found.push(Mismatch {
                    kind: Kind::Values,
                    detail: format!(
                        "row {i}: {} vs {}",
                        short(&base.rows[i]),
                        short(&them.rows[i])
                    ),
                });
            }
        }
        Order::Unordered => {
            ours.sort();
            theirs.sort();
            if ours != theirs {
                let (only_ours, only_theirs) = multiset_difference(&ours, &theirs);
                let mut detail = format!(
                    "as multisets: {} rows only in the baseline, {} only in the target",
                    only_ours.len(),
                    only_theirs.len()
                );
                if let Some(row) = only_ours.first() {
                    let _ = write!(detail, "; baseline has {}", short_canon(row));
                }
                if let Some(row) = only_theirs.first() {
                    let _ = write!(detail, "; target has {}", short_canon(row));
                }
                found.push(Mismatch {
                    kind: Kind::Values,
                    detail,
                });
            }
        }
    }
    found
}

/// Compares two answers in a named output format as bytes, after removing
/// `JSON*`'s `statistics` (times and read counts differ between runs). Errors
/// compare by code, as in [`compare`].
pub fn compare_bytes(
    baseline: &Result<Vec<u8>, Outcome>,
    other: &Result<Vec<u8>, Outcome>,
) -> Vec<Mismatch> {
    match (baseline, other) {
        (Ok(a), Ok(b)) => {
            let (a, b) = (strip_statistics(a), strip_statistics(b));
            if a == b {
                return Vec::new();
            }
            let at = a
                .iter()
                .zip(&b)
                .position(|(x, y)| x != y)
                .unwrap_or(a.len().min(b.len()));
            let from = at.saturating_sub(24);
            let show = |bytes: &[u8]| {
                let end = (at + 40).min(bytes.len());
                clip(
                    &format!("{:?}", String::from_utf8_lossy(&bytes[from.min(end)..end])),
                    120,
                )
            };
            vec![Mismatch {
                kind: Kind::Bytes,
                detail: format!(
                    "{} bytes vs {} bytes, first difference at byte {at}: {} vs {}",
                    a.len(),
                    b.len(),
                    show(&a),
                    show(&b)
                ),
            }]
        }
        (Err(Outcome::Error { code: a, .. }), Err(Outcome::Error { code: b, .. })) if a == b => {
            Vec::new()
        }
        (a, b) => {
            let side = |r: &Result<Vec<u8>, Outcome>| match r {
                Ok(bytes) => format!("{} bytes", bytes.len()),
                Err(e) => describe(e),
            };
            vec![Mismatch {
                kind: Kind::Error,
                detail: format!("baseline {}, target {}", side(a), side(b)),
            }]
        }
    }
}

/// Removes `,\n\n\t"statistics":\n\t{ … \n\t}` from a `JSON*` document; any
/// other answer comes back unchanged.
pub fn strip_statistics(document: &[u8]) -> Vec<u8> {
    const KEY: &[u8] = b",\n\n\t\"statistics\":\n\t{\n";
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    let Some(start) = find(document, KEY) else {
        return document.to_vec();
    };
    let rest = start + KEY.len();
    let Some(end) = find(&document[rest..], b"\n\t}").map(|e| rest + e + 3) else {
        return document.to_vec();
    };
    let mut out = document[..start].to_vec();
    out.extend_from_slice(&document[end..]);
    out
}

/// The rows of each sorted multiset that the other lacks, multiplicity counted.
fn multiset_difference<'a>(
    a: &'a [Vec<Canon>],
    b: &'a [Vec<Canon>],
) -> (Vec<&'a Vec<Canon>>, Vec<&'a Vec<Canon>>) {
    let (mut i, mut j) = (0, 0);
    let (mut only_a, mut only_b) = (Vec::new(), Vec::new());
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => {
                only_a.push(&a[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                only_b.push(&b[j]);
                j += 1;
            }
        }
    }
    only_a.extend(&a[i..]);
    only_b.extend(&b[j..]);
    (only_a, only_b)
}

fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Rows(r) => format!("{} rows", r.rows.len()),
        Outcome::Error { code, message } => format!("error {code} ({})", clip(message, 120)),
    }
}

fn short(row: &[Value]) -> String {
    clip(&Value::Array(row.to_vec()).to_string(), 160)
}

fn short_canon(row: &[Canon]) -> String {
    clip(&format!("{row:?}"), 160)
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_cases_compare_without_statistics() {
        let a =
            b"{\n\t\"rows\": 1,\n\n\t\"statistics\":\n\t{\n\t\t\"elapsed\": 0.1\n\t}\n}\n".to_vec();
        let b =
            b"{\n\t\"rows\": 1,\n\n\t\"statistics\":\n\t{\n\t\t\"elapsed\": 0.2\n\t}\n}\n".to_vec();
        assert!(compare_bytes(&Ok(a.clone()), &Ok(b)).is_empty());
        let c = b"{\n\t\"rows\": 2\n}\n".to_vec();
        let found = compare_bytes(&Ok(a), &Ok(c));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, Kind::Bytes);
        let err = |code| {
            Err(Outcome::Error {
                code,
                message: String::new(),
            })
        };
        assert!(compare_bytes(&err(48), &err(48)).is_empty());
        assert_eq!(
            compare_bytes(&Ok(Vec::new()), &err(48))[0].kind,
            Kind::Error
        );
    }
    use serde_json::json;

    #[test]
    fn float_formatting_is_hidden_to_the_declared_precision() {
        let p = Precision::default();
        assert_eq!(
            canon(&json!(0.300_000_000_000_000_04), "Float64", p),
            canon(&json!(0.3), "Float64", p)
        );
        assert_ne!(
            canon(&json!(0.3001), "Float64", p),
            canon(&json!(0.3), "Float64", p)
        );
        assert_eq!(
            canon(&json!(-0.0), "Float64", p),
            canon(&json!(0), "Float64", p)
        );
        assert_eq!(
            canon(&json!("nan"), "Float64", p),
            Canon::Float("nan".into())
        );
        assert_eq!(
            canon(&json!("-inf"), "Float32", p),
            Canon::Float("-inf".into())
        );
        assert_eq!(
            canon(&json!(0.1), "Float32", p),
            canon(&json!(0.100_000_001_490_116_12), "Float32", p)
        );
    }

    #[test]
    fn integer_and_decimal_quirks_are_hidden() {
        let p = Precision::default();
        assert_eq!(
            canon(&json!("18446744073709551615"), "UInt64", p),
            canon(&json!(18_446_744_073_709_551_615u64), "UInt64", p)
        );
        assert_eq!(
            canon(&json!(1.25), "Decimal(9, 4)", p),
            canon(&json!("1.2500"), "Decimal(9, 4)", p)
        );
        assert_eq!(
            canon(&json!("3.000"), "Decimal(9, 3)", p),
            Canon::Decimal("3".into())
        );
        // A string column keeps its text exactly.
        assert_ne!(
            canon(&json!("1"), "String", p),
            canon(&json!("1.0"), "String", p)
        );
    }
}
