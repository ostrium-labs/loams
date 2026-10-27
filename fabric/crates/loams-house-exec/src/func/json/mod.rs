//! JSON extraction (plan HE1 Task 33): the `JSONExtract*` family, `JSONHas`,
//! `JSONLength`, `JSONType`, `JSONExtractKeys`, `JSONExtractArrayRaw`, the
//! `simpleJSON*` (`visitParam*`) family, and `JSON_VALUE`, `JSON_QUERY` and
//! `JSON_EXISTS`, from `src/Functions/FunctionsJSON.h`,
//! `src/Functions/FunctionsVisitParam.h` and `src/Functions/JSONPath/`
//! (Apache-2.0). `JSONAllPaths` (for `JSON` columns) is Task 43's.
//!
//! * The `JSONExtract*` functions read the whole document; an invalid one,
//!   a missing path or a value of another kind gives the type's default
//!   (`''`, `0`). Path arguments are keys (strings) or 1-based positions
//!   (integers, negative from the end).
//! * The `simpleJSON*` functions do not parse: they find the first
//!   `"field":` anywhere in the text and read the value after it.
//! * `JSON_VALUE` takes a JSONPath (`$.a.b[0]`, `$['a']`) and returns a
//!   scalar's text, `''` for objects, arrays and missing values.

pub mod scan;

use loams_house_sql::Registry;
use loams_house_types::{ChError, ChType};

use crate::func::convert::cast::{convert, int_range};
use crate::func::convert::text::{parse_decimal, parse_float, parse_int};
use crate::func::convert::type_argument;
use crate::func::convert::value::{Value, build, default_value, read};
use crate::func::typed::{
    ChFn, Nulls, Sig, arity, const_str, illegal_type, is_integer, kernel, typer,
};
use scan::{Kind, Node, Number, Step, elements, find, members, number, parse, unescape};

/// `JSONType`'s result type.
pub fn json_type_enum() -> ChType {
    ChType::Enum8(vec![
        ("Null".into(), 0),
        ("String".into(), 34),
        ("Array".into(), 91),
        ("Object".into(), 123),
        ("Int64".into(), 105),
        ("UInt64".into(), 117),
        ("Double".into(), 100),
        ("Bool".into(), 98),
    ])
}

fn is_text(t: &ChType) -> bool {
    matches!(t, ChType::String | ChType::FixedString(_))
}

fn check_json_args(s: &Sig<'_>, paths: std::ops::Range<usize>) -> Result<(), ChError> {
    if !is_text(&s.types[0]) {
        return Err(illegal_type(s.name, 0, &s.types[0]));
    }
    for i in paths {
        let t = &s.types[i];
        if !is_text(t) && !is_integer(t) {
            return Err(illegal_type(s.name, i, t));
        }
    }
    Ok(())
}

/// The path of one row: `None` when a step is `NULL`.
fn steps(
    c: &crate::func::typed::Call<'_>,
    range: std::ops::Range<usize>,
    row: usize,
) -> Result<Option<Vec<Step>>, ChError> {
    let mut out = Vec::with_capacity(range.len());
    for i in range {
        match read(c.args[i].as_ref(), &c.types[i], row)? {
            Value::Str(b) => out.push(Step::Key(b)),
            Value::Int(n) => out.push(Step::Index(
                n.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
            )),
            _ => return Ok(None),
        }
    }
    Ok(Some(out))
}

type Extract = fn(Option<Node<'_>>, &ChType) -> Value;

/// A `JSON…(json, path…)` function whose result is `ret`, computed by `f`.
/// With `typed`, the last argument names the result type (`JSONExtract`).
fn json_function(name: &str, ret: Option<ChType>, f: Extract) -> ChFn {
    let typed = ret.is_none();
    let fixed = ret;
    ChFn::new(
        name,
        Nulls::Default,
        typer(move |s| {
            let min = if typed { 2 } else { 1 };
            arity(s, min, usize::MAX)?;
            let n = s.types.len();
            let path_end = if typed { n - 1 } else { n };
            check_json_args(s, 1..path_end)?;
            match &fixed {
                Some(t) => Ok(t.clone()),
                None => {
                    let t = type_argument(s, n - 1)?;
                    if matches!(t.base(), ChType::Map(..)) {
                        return Err(ChError::unsupported("JSONExtract into a Map"));
                    }
                    Ok(t)
                }
            }
        }),
        kernel(move |c| {
            let n = c.args.len();
            let path_end = if typed { n - 1 } else { n };
            let mut out = Vec::with_capacity(c.rows);
            for row in 0..c.rows {
                let doc = match read(c.args[0].as_ref(), &c.types[0], row)? {
                    Value::Str(b) => b,
                    _ => {
                        out.push(default_value(c.ret));
                        continue;
                    }
                };
                let node = match steps(c, 1..path_end, row)? {
                    Some(path) => parse(&doc).and_then(|d| find(d, &path)),
                    None => None,
                };
                out.push(f(node, c.ret));
            }
            build(c.ret, &out)
        }),
    )
}

fn json_string(node: Option<Node<'_>>) -> Option<Vec<u8>> {
    let node = node?;
    if node.kind == Kind::String {
        unescape(node.raw)
    } else {
        None
    }
}

/// A node as an integer of `t` (`None` when it is not one, or does not fit).
fn as_int(node: Node<'_>, t: &ChType) -> Option<i128> {
    let (min, max) = int_range(t)?;
    let v = match node.kind {
        Kind::Number => match number(node.raw)? {
            Number::Int(i) => i128::from(i),
            Number::UInt(u) => i128::from(u),
            Number::Double(d) => {
                if d.fract() != 0.0 || !d.is_finite() {
                    return None;
                }
                d as i128
            }
        },
        Kind::Bool => i128::from(node.raw == b"true"),
        Kind::String => parse_int(&unescape(node.raw)?)?,
        _ => return None,
    };
    (min..=max).contains(&v).then_some(v)
}

fn as_float(node: Node<'_>) -> Option<f64> {
    match node.kind {
        Kind::Number => match number(node.raw)? {
            Number::Int(i) => Some(i as f64),
            Number::UInt(u) => Some(u as f64),
            Number::Double(d) => Some(d),
        },
        Kind::Bool => Some(if node.raw == b"true" { 1.0 } else { 0.0 }),
        Kind::String => parse_float(&unescape(node.raw)?),
        _ => None,
    }
}

/// `JSONExtract(…, 'T')`: the node as a value of `t`; the default (or `NULL`
/// for `Nullable`) when it is missing or of another kind.
pub fn json_value(node: Option<Node<'_>>, t: &ChType) -> Value {
    match t {
        ChType::Nullable(inner) => match node {
            Some(n) if n.kind != Kind::Null => convert_node(n, inner).unwrap_or(Value::Null),
            _ => Value::Null,
        },
        _ => node
            .and_then(|n| convert_node(n, t))
            .unwrap_or_else(|| default_value(t)),
    }
}

fn convert_node(node: Node<'_>, t: &ChType) -> Option<Value> {
    Some(match t {
        ChType::LowCardinality(inner) => convert_node(node, inner)?,
        ChType::Nullable(_) => json_value(Some(node), t),
        ChType::UInt8
        | ChType::UInt16
        | ChType::UInt32
        | ChType::UInt64
        | ChType::Int8
        | ChType::Int16
        | ChType::Int32
        | ChType::Int64 => Value::Int(as_int(node, t)?),
        ChType::Float32 | ChType::Float64 => Value::Float(as_float(node)?),
        ChType::Bool => match node.kind {
            Kind::Bool => Value::Bool(node.raw == b"true"),
            Kind::Number => Value::Bool(as_float(node)? != 0.0),
            _ => return None,
        },
        ChType::String => Value::Str(json_string(Some(node))?),
        ChType::Decimal { scale, .. } => {
            let text = match node.kind {
                Kind::Number => node.raw.to_vec(),
                Kind::String => unescape(node.raw)?,
                _ => return None,
            };
            let u = parse_decimal(&text, *scale)?.ok()?;
            convert(
                Value::Dec(u, *scale),
                &ChType::Decimal {
                    precision: 38,
                    scale: *scale,
                },
                t,
                false,
            )
            .ok()?
        }
        ChType::Enum8(_) | ChType::Enum16(_) => {
            let v = match node.kind {
                Kind::String => Value::Str(unescape(node.raw)?),
                Kind::Number => Value::Int(as_int(node, &ChType::Int64)?),
                _ => return None,
            };
            let from = if matches!(v, Value::Str(_)) {
                ChType::String
            } else {
                ChType::Int64
            };
            convert(v, &from, t, false).ok()?
        }
        ChType::Array(inner) => {
            if node.kind != Kind::Array {
                return None;
            }
            Value::Array(
                elements(node)
                    .into_iter()
                    .map(|e| json_value(Some(e), inner))
                    .collect(),
            )
        }
        ChType::Tuple(fields) => {
            let named = fields.first().is_some_and(|(n, _)| n.is_some());
            let values: Vec<Value> = match node.kind {
                Kind::Object if named => {
                    let m = members(node);
                    fields
                        .iter()
                        .map(|(name, ft)| {
                            let key = name.clone().unwrap_or_default().into_bytes();
                            json_value(m.iter().find(|(k, _)| *k == key).map(|(_, v)| *v), ft)
                        })
                        .collect()
                }
                Kind::Object => {
                    let m = members(node);
                    fields
                        .iter()
                        .enumerate()
                        .map(|(i, (_, ft))| json_value(m.get(i).map(|(_, v)| *v), ft))
                        .collect()
                }
                Kind::Array => {
                    let e = elements(node);
                    fields
                        .iter()
                        .enumerate()
                        .map(|(i, (_, ft))| json_value(e.get(i).copied(), ft))
                        .collect()
                }
                _ => return None,
            };
            Value::Tuple(values)
        }
        other => {
            // Dates, times, UUIDs and addresses from their text.
            let text = json_string(Some(node))?;
            convert(Value::Str(text), &ChType::String, other, false).ok()?
        }
    })
}

/// A JSONPath step list (`$.a.b[0]`, `$['a'][1]`): positions are 0-based in
/// JSONPath and 1-based here.
pub fn parse_json_path(path: &str) -> Result<Vec<Step>, ChError> {
    let bad = || ChError::bad_arguments(format!("Unable to parse JSONPath '{path}'"));
    let b = path.trim().as_bytes();
    if b.first() != Some(&b'$') {
        return Err(bad());
    }
    let mut out = Vec::new();
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b'.' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && b[j] != b'.' && b[j] != b'[' {
                    j += 1;
                }
                if j == start {
                    return Err(bad());
                }
                let key = &b[start..j];
                let key = key
                    .strip_prefix(b"\"")
                    .and_then(|k| k.strip_suffix(b"\""))
                    .unwrap_or(key);
                out.push(Step::Key(key.to_vec()));
                i = j;
            }
            b'[' => {
                let close = b[i..].iter().position(|c| *c == b']').ok_or_else(bad)? + i;
                let inner = path.trim()[i + 1..close].trim();
                let quoted = inner
                    .strip_prefix('\'')
                    .and_then(|k| k.strip_suffix('\''))
                    .or_else(|| inner.strip_prefix('"').and_then(|k| k.strip_suffix('"')));
                match quoted {
                    Some(key) => out.push(Step::Key(key.as_bytes().to_vec())),
                    None => {
                        let n: i64 = inner.parse().map_err(|_| bad())?;
                        if n < 0 {
                            return Err(bad());
                        }
                        out.push(Step::Index(n + 1));
                    }
                }
                i = close + 1;
            }
            _ => return Err(bad()),
        }
    }
    Ok(out)
}

/// Where `"field":` first appears in `doc`, and the value's start.
fn visit(doc: &[u8], field: &[u8]) -> Option<usize> {
    let needle = [b"\"".as_slice(), field, b"\":".as_slice()].concat();
    doc.windows(needle.len())
        .position(|w| w == needle.as_slice())
        .map(|p| p + needle.len())
}

/// The numeric prefix of `s` (after an optional opening quote).
fn numeric_prefix(s: &[u8], float: bool) -> &[u8] {
    let s = s.strip_prefix(b"\"").unwrap_or(s);
    let mut j = 0;
    while j < s.len() {
        let c = s[j];
        let ok = c.is_ascii_digit()
            || (j == 0 && (c == b'-' || c == b'+'))
            || (float && (c == b'.' || c == b'e' || c == b'E'))
            || (float && j > 0 && (c == b'-' || c == b'+') && matches!(s[j - 1], b'e' | b'E'));
        if !ok {
            break;
        }
        j += 1;
    }
    &s[..j]
}

type Visit = fn(Option<&[u8]>, &ChType) -> Value;

fn simple_function(name: &str, ret: ChType, f: Visit) -> ChFn {
    ChFn::new(
        name,
        Nulls::Default,
        typer(move |s| {
            arity(s, 2, 2)?;
            if !is_text(&s.types[0]) {
                return Err(illegal_type(s.name, 0, &s.types[0]));
            }
            if !is_text(&s.types[1]) {
                return Err(illegal_type(s.name, 1, &s.types[1]));
            }
            Ok(ret.clone())
        }),
        kernel(move |c| {
            let mut out = Vec::with_capacity(c.rows);
            for row in 0..c.rows {
                let doc = read(c.args[0].as_ref(), &c.types[0], row)?;
                let field = read(c.args[1].as_ref(), &c.types[1], row)?;
                let v = match (doc, field) {
                    (Value::Str(doc), Value::Str(field)) => {
                        let at = visit(&doc, &field);
                        f(at.map(|p| &doc[p..]), c.ret)
                    }
                    _ => default_value(c.ret),
                };
                out.push(v);
            }
            build(c.ret, &out)
        }),
    )
}

fn json_path_function(name: &str, ret: ChType, f: Extract) -> ChFn {
    ChFn::new(
        name,
        Nulls::Default,
        typer(move |s| {
            arity(s, 2, 2)?;
            if !is_text(&s.types[0]) {
                return Err(illegal_type(s.name, 0, &s.types[0]));
            }
            let path = const_str(s.consts.get(1).and_then(Option::as_ref)).ok_or_else(|| {
                ChError::illegal_type_of_argument(format!(
                    "JSONPath of function {} must be a constant string",
                    s.name
                ))
            })?;
            parse_json_path(&path)?;
            Ok(ret.clone())
        }),
        kernel(move |c| {
            let path = const_str(c.consts.get(1).and_then(Option::as_ref)).unwrap_or_default();
            let path = parse_json_path(&path)?;
            let mut out = Vec::with_capacity(c.rows);
            for row in 0..c.rows {
                let node_value = match read(c.args[0].as_ref(), &c.types[0], row)? {
                    Value::Str(doc) => {
                        let node = parse(&doc).and_then(|d| find(d, &path));
                        f(node, c.ret)
                    }
                    _ => default_value(c.ret),
                };
                out.push(node_value);
            }
            build(c.ret, &out)
        }),
    )
}

fn string_value(b: Vec<u8>) -> Value {
    Value::Str(b)
}

/// Registers the JSON functions.
pub fn register(r: &mut Registry) {
    json_function("JSONHas", Some(ChType::UInt8), |n, _| {
        Value::Int(i128::from(n.is_some()))
    })
    .register(r);
    json_function("JSONLength", Some(ChType::UInt64), |n, _| {
        Value::Int(match n {
            Some(n) if n.kind == Kind::Array => elements(n).len() as i128,
            Some(n) if n.kind == Kind::Object => members(n).len() as i128,
            _ => 0,
        })
    })
    .register(r);
    json_function("JSONType", Some(json_type_enum()), |n, _| {
        let (code, name) = match n.map(|n| (n.kind, n)) {
            None => (0, "Null"),
            Some((Kind::Null, _)) => (0, "Null"),
            Some((Kind::String, _)) => (34, "String"),
            Some((Kind::Array, _)) => (91, "Array"),
            Some((Kind::Object, _)) => (123, "Object"),
            Some((Kind::Bool, _)) => (98, "Bool"),
            Some((Kind::Number, n)) => match number(n.raw) {
                Some(Number::Int(_)) => (105, "Int64"),
                Some(Number::UInt(_)) => (117, "UInt64"),
                _ => (100, "Double"),
            },
        };
        Value::Enum(code, name.to_string())
    })
    .register(r);
    json_function("JSONExtractString", Some(ChType::String), |n, _| {
        string_value(json_string(n).unwrap_or_default())
    })
    .register(r);
    json_function("JSONExtractInt", Some(ChType::Int64), |n, t| {
        Value::Int(n.and_then(|n| as_int(n, t)).unwrap_or(0))
    })
    .register(r);
    json_function("JSONExtractUInt", Some(ChType::UInt64), |n, t| {
        Value::Int(n.and_then(|n| as_int(n, t)).unwrap_or(0))
    })
    .register(r);
    json_function("JSONExtractFloat", Some(ChType::Float64), |n, _| {
        Value::Float(n.and_then(as_float).unwrap_or(0.0))
    })
    .register(r);
    json_function("JSONExtractBool", Some(ChType::UInt8), |n, _| {
        Value::Int(i128::from(
            n.is_some_and(|n| n.kind == Kind::Bool && n.raw == b"true"),
        ))
    })
    .register(r);
    json_function("JSONExtractRaw", Some(ChType::String), |n, _| {
        string_value(n.map(|n| n.raw.to_vec()).unwrap_or_default())
    })
    .register(r);
    json_function(
        "JSONExtractKeys",
        Some(ChType::Array(Box::new(ChType::String))),
        |n, _| {
            Value::Array(
                n.map(members)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(k, _)| Value::Str(k))
                    .collect(),
            )
        },
    )
    .register(r);
    json_function(
        "JSONExtractArrayRaw",
        Some(ChType::Array(Box::new(ChType::String))),
        |n, _| {
            Value::Array(
                n.map(elements)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|e| Value::Str(e.raw.to_vec()))
                    .collect(),
            )
        },
    )
    .register(r);
    json_function("JSONExtract", None, json_value).register(r);

    simple_function("simpleJSONHas", ChType::UInt8, |at, _| {
        Value::Int(i128::from(at.is_some()))
    })
    .register(r);
    simple_function("simpleJSONExtractString", ChType::String, |at, _| {
        let s = at
            .filter(|s| s.first() == Some(&b'"'))
            .and_then(|s| scan::value(s, 0, 0))
            .filter(|(n, _)| n.kind == Kind::String)
            .and_then(|(n, _)| unescape(n.raw));
        Value::Str(s.unwrap_or_default())
    })
    .register(r);
    simple_function("simpleJSONExtractRaw", ChType::String, |at, _| {
        let raw = at
            .and_then(|s| scan::value(s, 0, 0))
            .map(|(n, _)| n.raw.to_vec());
        Value::Str(raw.unwrap_or_default())
    })
    .register(r);
    simple_function("simpleJSONExtractInt", ChType::Int64, |at, _| {
        let v = at
            .and_then(|s| parse_int(numeric_prefix(s, false)))
            .unwrap_or(0);
        Value::Int(i128::from(v as i64))
    })
    .register(r);
    simple_function("simpleJSONExtractUInt", ChType::UInt64, |at, _| {
        let v = at
            .and_then(|s| parse_int(numeric_prefix(s, false)))
            .filter(|v| *v >= 0)
            .unwrap_or(0);
        Value::Int(i128::from(v as u64))
    })
    .register(r);
    simple_function("simpleJSONExtractFloat", ChType::Float64, |at, _| {
        let v = at
            .and_then(|s| parse_float(numeric_prefix(s, true)))
            .unwrap_or(0.0);
        Value::Float(v)
    })
    .register(r);
    simple_function("simpleJSONExtractBool", ChType::UInt8, |at, _| {
        Value::Int(i128::from(at.is_some_and(|s| s.starts_with(b"true"))))
    })
    .register(r);

    json_path_function("JSON_VALUE", ChType::String, |n, _| {
        Value::Str(match n {
            Some(n) if n.kind == Kind::String => unescape(n.raw).unwrap_or_default(),
            Some(n) if matches!(n.kind, Kind::Number | Kind::Bool | Kind::Null) => n.raw.to_vec(),
            _ => Vec::new(),
        })
    })
    .register(r);
    json_path_function("JSON_QUERY", ChType::String, |n, _| {
        Value::Str(match n {
            Some(n) => [b"[".as_slice(), n.raw, b"]".as_slice()].concat(),
            None => Vec::new(),
        })
    })
    .register(r);
    json_path_function("JSON_EXISTS", ChType::UInt8, |n, _| {
        Value::Int(i128::from(n.is_some()))
    })
    .register(r);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use loams_house_types::parse as parse_type;

    fn doc(text: &str, path: &[Step]) -> Option<Node<'static>> {
        let text: &'static str = Box::leak(text.to_string().into_boxed_str());
        parse(text.as_bytes()).and_then(|d| find(d, path))
    }

    fn key(k: &str) -> Step {
        Step::Key(k.as_bytes().to_vec())
    }

    #[test]
    fn extract_typed_values() {
        let n = doc(r#"{"a": [1, "2", 3.5]}"#, &[key("a")]);
        assert_eq!(
            json_value(n, &parse_type("Array(Nullable(Int32))").unwrap()),
            Value::Array(vec![Value::Int(1), Value::Int(2), Value::Null])
        );
        assert_eq!(
            json_value(n, &parse_type("Array(Float64)").unwrap()),
            Value::Array(vec![
                Value::Float(1.0),
                Value::Float(2.0),
                Value::Float(3.5)
            ])
        );
        assert_eq!(json_value(None, &ChType::UInt8), Value::Int(0));
        assert_eq!(
            json_value(None, &parse_type("Nullable(String)").unwrap()),
            Value::Null
        );
        let t = doc(r#"{"x": {"a": 1, "b": "s"}}"#, &[key("x")]);
        assert_eq!(
            json_value(t, &parse_type("Tuple(b String, a UInt8)").unwrap()),
            Value::Tuple(vec![Value::Str(b"s".to_vec()), Value::Int(1)])
        );
    }

    #[test]
    fn integers_must_fit() {
        let n = doc(r#"{"a": -1}"#, &[key("a")]).unwrap();
        assert_eq!(as_int(n, &ChType::UInt64), None);
        assert_eq!(as_int(n, &ChType::Int64), Some(-1));
        let f = doc(r#"{"a": 1.5}"#, &[key("a")]).unwrap();
        assert_eq!(as_int(f, &ChType::Int64), None);
    }

    #[test]
    fn json_paths_parse() {
        assert_eq!(
            parse_json_path("$.a.b[0]").unwrap(),
            vec![key("a"), key("b"), Step::Index(1)]
        );
        assert_eq!(parse_json_path("$['a']").unwrap(), vec![key("a")]);
        assert_eq!(parse_json_path("$").unwrap(), vec![]);
        assert_eq!(parse_json_path("a.b").unwrap_err().code, 36);
        assert_eq!(parse_json_path("$.").unwrap_err().code, 36);
    }

    #[test]
    fn simple_extraction_scans_the_text() {
        let d = br#"{"a":{"b":"x\"y","n":-12.5e1},"b":2}"#;
        let at = visit(d, b"b").unwrap();
        assert_eq!(&d[at..at + 1], b"\"");
        assert!(visit(d, b"zz").is_none());
        let n = visit(d, b"n").unwrap();
        assert_eq!(numeric_prefix(&d[n..], false), b"-12");
        assert_eq!(numeric_prefix(&d[n..], true), b"-12.5e1");
    }
}
