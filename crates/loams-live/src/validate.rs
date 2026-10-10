//! Validators (LV1 plan Task 4; design §45 §4): the vocabulary function
//! arguments use now and table schemas use from Task 8, and [`check`].
//!
//! A [`Validator`] is `v.null | int64 | float64 | boolean | string | bytes |
//! array | object | literal | union | optional | any | id(table)`, as
//! `loams:server`'s `v` builds it. Values match strictly: an `int64` is a
//! [`LiveValue::I64`] (a JavaScript `bigint`), a `float64` a
//! [`LiveValue::F64`] (a number); an object has exactly its fields, except
//! that an `optional` field may be missing; an `id(table)` is a document id
//! in its text form (checksum included) of that table.
//!
//! A failed check returns every [`FieldViolation`] (up to
//! [`MAX_VIOLATIONS`]), each naming the field path. Messages name types and
//! the validator's own literals, never the checked value (no user data in
//! errors).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{DocId, LiveValue, TableId};

/// The most violations one check reports.
pub const MAX_VIOLATIONS: usize = 16;

/// A validator of [`LiveValue`]s.
#[derive(Debug, Clone, PartialEq)]
pub enum Validator {
    Null,
    Int64,
    Float64,
    Boolean,
    String,
    Bytes,
    /// An array whose elements all match.
    Array(Box<Validator>),
    /// An object with exactly these fields; a field whose validator is
    /// [`Validator::Optional`] may be missing.
    Object(BTreeMap<String, Validator>),
    /// Exactly this scalar (null, boolean, int64, float64 or string).
    Literal(LiveValue),
    /// Any of the members.
    Union(Vec<Validator>),
    /// An object field that may be missing; anywhere else it is its inner
    /// validator (there is no "missing" outside an object).
    Optional(Box<Validator>),
    Any,
    /// A document id of the named table.
    Id(String),
}

impl Validator {
    /// The validator's name in messages (`v.<name>()`).
    pub fn name(&self) -> &'static str {
        match self {
            Validator::Null => "null",
            Validator::Int64 => "int64",
            Validator::Float64 => "float64",
            Validator::Boolean => "boolean",
            Validator::String => "string",
            Validator::Bytes => "bytes",
            Validator::Array(_) => "array",
            Validator::Object(_) => "object",
            Validator::Literal(_) => "literal",
            Validator::Union(_) => "union",
            Validator::Optional(_) => "optional",
            Validator::Any => "any",
            Validator::Id(_) => "id",
        }
    }

    /// The tables its `id(table)` validators name, so a caller can resolve
    /// them for [`check_ids`].
    pub fn id_tables(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        self.collect_tables(&mut out);
        out
    }

    fn collect_tables(&self, out: &mut BTreeSet<String>) {
        match self {
            Validator::Id(table) => {
                out.insert(table.clone());
            }
            Validator::Array(v) | Validator::Optional(v) => v.collect_tables(out),
            Validator::Object(fields) => fields.values().for_each(|v| v.collect_tables(out)),
            Validator::Union(members) => members.iter().for_each(|v| v.collect_tables(out)),
            _ => {}
        }
    }
}

/// One step of a field path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathElem {
    Field(String),
    Index(usize),
}

/// A value that does not match its validator at `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldViolation {
    pub path: Vec<PathElem>,
    pub message: String,
}

impl fmt::Display for FieldViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", render_path(&self.path), self.message)
    }
}

/// A path as JavaScript would write it from the value `$`: `$.a[3]["b c"]`.
pub fn render_path(path: &[PathElem]) -> String {
    let mut out = String::from("$");
    for elem in path {
        match elem {
            PathElem::Index(i) => out.push_str(&format!("[{i}]")),
            PathElem::Field(name) if is_identifier(name) => {
                out.push('.');
                out.push_str(name);
            }
            PathElem::Field(name) => out.push_str(&format!("[{name:?}]")),
        }
    }
    out
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// The violations as one message: the first few, then how many more.
pub fn describe(violations: &[FieldViolation]) -> String {
    const SHOWN: usize = 4;
    let mut parts: Vec<String> = violations
        .iter()
        .take(SHOWN)
        .map(|v| v.to_string())
        .collect();
    if violations.len() > SHOWN {
        parts.push(format!("and {} more", violations.len() - SHOWN));
    }
    parts.join("; ")
}

/// Checks `value` against `validator`. An `id(table)` is checked as a
/// well-formed document id only; [`check_ids`] also checks its table.
pub fn check(validator: &Validator, value: &LiveValue) -> Result<(), Vec<FieldViolation>> {
    Checker::new(None).finish(validator, value)
}

/// Like [`check`], with each `id(table)` also checked against `tables`, the
/// ids of the tables [`Validator::id_tables`] names. A table missing from
/// `tables` does not exist, so no id is of it.
pub fn check_ids(
    validator: &Validator,
    value: &LiveValue,
    tables: &BTreeMap<String, TableId>,
) -> Result<(), Vec<FieldViolation>> {
    Checker::new(Some(tables)).finish(validator, value)
}

struct Checker<'t> {
    tables: Option<&'t BTreeMap<String, TableId>>,
    path: Vec<PathElem>,
    out: Vec<FieldViolation>,
}

impl<'t> Checker<'t> {
    fn new(tables: Option<&'t BTreeMap<String, TableId>>) -> Self {
        Checker {
            tables,
            path: Vec::new(),
            out: Vec::new(),
        }
    }

    fn finish(
        mut self,
        validator: &Validator,
        value: &LiveValue,
    ) -> Result<(), Vec<FieldViolation>> {
        self.run(validator, value);
        if self.out.is_empty() {
            Ok(())
        } else {
            Err(self.out)
        }
    }

    fn violation(&mut self, message: String) {
        if self.out.len() < MAX_VIOLATIONS {
            self.out.push(FieldViolation {
                path: self.path.clone(),
                message,
            });
        }
    }

    fn mismatch(&mut self, validator: &Validator, value: &LiveValue) {
        self.violation(format!(
            "expected {}, got {}",
            validator.name(),
            value.type_name()
        ));
    }

    fn at(&mut self, elem: PathElem, validator: &Validator, value: &LiveValue) {
        self.path.push(elem);
        self.run(validator, value);
        self.path.pop();
    }

    fn run(&mut self, validator: &Validator, value: &LiveValue) {
        if self.out.len() >= MAX_VIOLATIONS {
            return;
        }
        let matches = match (validator, value) {
            (Validator::Any, _)
            | (Validator::Null, LiveValue::Null)
            | (Validator::Int64, LiveValue::I64(_))
            | (Validator::Float64, LiveValue::F64(_))
            | (Validator::Boolean, LiveValue::Bool(_))
            | (Validator::String, LiveValue::Str(_))
            | (Validator::Bytes, LiveValue::Bytes(_)) => true,
            (Validator::Optional(inner), value) => {
                self.run(inner, value);
                true
            }
            (Validator::Literal(literal), value) => {
                if literal != value {
                    self.violation(format!("expected the literal {}", literal_text(literal)));
                }
                true
            }
            (Validator::Id(table), LiveValue::Str(text)) => {
                self.id(table, text);
                true
            }
            (Validator::Array(element), LiveValue::Array(items)) => {
                for (i, item) in items.iter().enumerate() {
                    self.at(PathElem::Index(i), element, item);
                }
                true
            }
            (Validator::Object(fields), LiveValue::Object(present)) => {
                self.object(fields, present);
                true
            }
            (Validator::Union(members), value) => {
                let tables = self.tables;
                if !members
                    .iter()
                    .any(|m| Checker::new(tables).finish(m, value).is_ok())
                {
                    self.violation(format!(
                        "matches none of the union's {} members",
                        members.len()
                    ));
                }
                true
            }
            _ => false,
        };
        if !matches {
            self.mismatch(validator, value);
        }
    }

    fn id(&mut self, table: &str, text: &str) {
        let Ok(id) = text.parse::<DocId>() else {
            self.violation(format!(
                "expected an id of table '{table}', got a string that is not a document id"
            ));
            return;
        };
        let Some(tables) = self.tables else {
            return;
        };
        match tables.get(table) {
            Some(t) if *t == id.table => {}
            Some(_) => self.violation(format!(
                "expected an id of table '{table}', got an id of another table"
            )),
            None => self.violation(format!(
                "expected an id of table '{table}', which does not exist"
            )),
        }
    }

    fn object(
        &mut self,
        fields: &BTreeMap<String, Validator>,
        present: &BTreeMap<String, LiveValue>,
    ) {
        for name in present.keys().filter(|k| !fields.contains_key(*k)) {
            self.path.push(PathElem::Field(name.clone()));
            self.violation("unknown field".into());
            self.path.pop();
        }
        for (name, validator) in fields {
            let (inner, optional) = match validator {
                Validator::Optional(inner) => (&**inner, true),
                v => (v, false),
            };
            match present.get(name) {
                Some(item) => self.at(PathElem::Field(name.clone()), inner, item),
                None if optional => {}
                None => {
                    self.path.push(PathElem::Field(name.clone()));
                    self.violation("missing required field".into());
                    self.path.pop();
                }
            }
        }
    }
}

/// A literal as JavaScript writes it.
fn literal_text(v: &LiveValue) -> String {
    match v {
        LiveValue::Null => "null".into(),
        LiveValue::Bool(b) => b.to_string(),
        LiveValue::I64(i) => format!("{i}n"),
        LiveValue::F64(f) => f.to_string(),
        LiveValue::Str(s) => format!("{s:?}"),
        other => other.type_name().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pairs: &[(&str, LiveValue)]) -> LiveValue {
        LiveValue::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    fn paths(r: Result<(), Vec<FieldViolation>>) -> Vec<String> {
        r.err()
            .unwrap_or_default()
            .iter()
            .map(|v| render_path(&v.path))
            .collect()
    }

    #[test]
    fn objects_have_exactly_their_fields() {
        let v = Validator::Object(BTreeMap::from([
            ("a".to_string(), Validator::Int64),
            (
                "b".to_string(),
                Validator::Optional(Box::new(Validator::String)),
            ),
        ]));
        assert!(check(&v, &obj(&[("a", LiveValue::I64(1))])).is_ok());
        assert_eq!(
            paths(check(
                &v,
                &obj(&[("b", LiveValue::Null), ("c", LiveValue::Null)])
            )),
            vec!["$.c", "$.a", "$.b"]
        );
    }

    #[test]
    fn ids_are_checked_for_form_and_table() {
        let id = DocId::random(TableId(3)).expect("an id").to_string();
        let v = Validator::Id("users".into());
        assert!(check(&v, &LiveValue::Str(id.clone())).is_ok());
        assert!(check(&v, &LiveValue::Str("x".into())).is_err());
        let users = BTreeMap::from([("users".to_string(), TableId(3))]);
        let posts = BTreeMap::from([("users".to_string(), TableId(4))]);
        assert!(check_ids(&v, &LiveValue::Str(id.clone()), &users).is_ok());
        assert!(check_ids(&v, &LiveValue::Str(id.clone()), &posts).is_err());
        assert!(check_ids(&v, &LiveValue::Str(id), &BTreeMap::new()).is_err());
        let nested = Validator::Union(vec![
            Validator::Array(Box::new(Validator::Id("a".into()))),
            Validator::Id("b".into()),
        ]);
        assert_eq!(
            nested.id_tables(),
            BTreeSet::from(["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn violations_are_capped_and_never_show_the_value() {
        let v = Validator::Array(Box::new(Validator::Int64));
        let value = LiveValue::Array(vec![LiveValue::Str("secret".into()); 100]);
        let violations = check(&v, &value).expect_err("all strings");
        assert_eq!(violations.len(), MAX_VIOLATIONS);
        let text = describe(&violations);
        assert!(!text.contains("secret"), "{text}");
        assert!(text.ends_with("and 12 more"), "{text}");
        let lit = Validator::Literal(LiveValue::Str("on".into()));
        let text = describe(&check(&lit, &LiveValue::Str("secret".into())).expect_err("no"));
        assert_eq!(text, "$: expected the literal \"on\"");
    }
}
