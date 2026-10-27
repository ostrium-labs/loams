//! A parsed query checked against the namespace's search attributes and
//! turned into a typed filter (D2074): every name resolved to a column of
//! the visibility row or a key of its `search_attrs`, every value converted
//! to the attribute's type.

use std::collections::BTreeMap;

use chrono::DateTime;

use crate::query::{self, CmpOp, Expr, Lit, Query};

/// `temporal.api.enums.v1.IndexedValueType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ValueType {
    Text,
    Keyword,
    Int,
    Double,
    Bool,
    Datetime,
    KeywordList,
}

impl ValueType {
    /// From the proto value.
    pub fn from_i32(v: i32) -> Option<ValueType> {
        Some(match v {
            1 => ValueType::Text,
            2 => ValueType::Keyword,
            3 => ValueType::Int,
            4 => ValueType::Double,
            5 => ValueType::Bool,
            6 => ValueType::Datetime,
            7 => ValueType::KeywordList,
            _ => return None,
        })
    }

    /// The proto value.
    pub fn to_i32(self) -> i32 {
        match self {
            ValueType::Text => 1,
            ValueType::Keyword => 2,
            ValueType::Int => 3,
            ValueType::Double => 4,
            ValueType::Bool => 5,
            ValueType::Datetime => 6,
            ValueType::KeywordList => 7,
        }
    }

    /// The name the SDKs put in a payload's `type` metadata.
    pub fn name(self) -> &'static str {
        match self {
            ValueType::Text => "Text",
            ValueType::Keyword => "Keyword",
            ValueType::Int => "Int",
            ValueType::Double => "Double",
            ValueType::Bool => "Bool",
            ValueType::Datetime => "Datetime",
            ValueType::KeywordList => "KeywordList",
        }
    }
}

/// A column of the visibility row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    WorkflowId,
    RunId,
    WorkflowType,
    ExecutionStatus,
    TaskQueue,
    StartTime,
    CloseTime,
    ExecutionTime,
    /// `CloseTime - StartTime`, null while running.
    ExecutionDuration,
}

impl Column {
    /// The column's type as a search attribute.
    pub fn value_type(self) -> ValueType {
        match self {
            Column::WorkflowId
            | Column::RunId
            | Column::WorkflowType
            | Column::ExecutionStatus
            | Column::TaskQueue => ValueType::Keyword,
            Column::StartTime | Column::CloseTime | Column::ExecutionTime => ValueType::Datetime,
            // Temporal lists it as Int (nanoseconds); values may be durations.
            Column::ExecutionDuration => ValueType::Int,
        }
    }
}

/// The system attributes the gateway stores as columns.
pub const COLUMNS: &[(&str, Column)] = &[
    ("WorkflowId", Column::WorkflowId),
    ("RunId", Column::RunId),
    ("WorkflowType", Column::WorkflowType),
    ("ExecutionStatus", Column::ExecutionStatus),
    ("TaskQueue", Column::TaskQueue),
    ("StartTime", Column::StartTime),
    ("CloseTime", Column::CloseTime),
    ("ExecutionTime", Column::ExecutionTime),
    ("ExecutionDuration", Column::ExecutionDuration),
];

/// System attributes kept in `search_attrs` (written by the gateway, not
/// registered by users).
pub const SYSTEM_JSON: &[(&str, ValueType)] = &[
    ("TemporalScheduledById", ValueType::Keyword),
    ("TemporalScheduledStartTime", ValueType::Datetime),
    ("TemporalSchedulePaused", ValueType::Bool),
    ("TemporalChangeVersion", ValueType::KeywordList),
    ("BuildIds", ValueType::KeywordList),
    ("BatcherUser", ValueType::Keyword),
];

/// Temporal's system attributes the gateway does not store; a query on one
/// is refused rather than answered wrongly.
pub const UNSUPPORTED: &[&str] = &[
    "HistoryLength",
    "HistorySizeBytes",
    "StateTransitionCount",
    "ParentWorkflowId",
    "ParentRunId",
    "RootWorkflowId",
    "RootRunId",
    "TemporalNamespaceDivision",
    "TemporalWorkerDeployment",
    "TemporalWorkerDeploymentVersion",
    "TemporalWorkflowVersioningBehavior",
    "TemporalPauseInfo",
    "TemporalReportedProblems",
];

/// Every system attribute and its type (columns and system JSON keys).
pub fn system_attributes() -> BTreeMap<String, ValueType> {
    COLUMNS
        .iter()
        .map(|(n, c)| ((*n).to_string(), c.value_type()))
        .chain(SYSTEM_JSON.iter().map(|(n, t)| ((*n).to_string(), *t)))
        .collect()
}

/// Whether `name` is a system attribute (custom ones may not reuse it).
pub fn is_system(name: &str) -> bool {
    COLUMNS.iter().any(|(n, _)| *n == name)
        || SYSTEM_JSON.iter().any(|(n, _)| *n == name)
        || UNSUPPORTED.contains(&name)
}

/// `WorkflowExecutionStatus` names.
pub const STATUSES: &[(&str, i32)] = &[
    ("Running", 1),
    ("Completed", 2),
    ("Failed", 3),
    ("Canceled", 4),
    ("Terminated", 5),
    ("ContinuedAsNew", 6),
    ("TimedOut", 7),
];

/// A status's name.
pub fn status_name(v: i32) -> &'static str {
    STATUSES
        .iter()
        .find(|(_, s)| *s == v)
        .map_or("Unspecified", |(n, _)| n)
}

/// Where an attribute lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Column(Column),
    /// A key of `search_attrs`.
    Json {
        key: String,
        ty: ValueType,
    },
}

impl Field {
    pub fn value_type(&self) -> ValueType {
        match self {
            Field::Column(c) => c.value_type(),
            Field::Json { ty, .. } => *ty,
        }
    }
}

/// A typed value. Times and durations are Unix milliseconds.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    TimeMs(i64),
}

/// A typed predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum Pred {
    Cmp(CmpOp, Value),
    In(Vec<Value>),
    Between(Value, Value),
    StartsWith(String),
    IsNull,
    /// Text `=`: every word of the value appears.
    TextMatch(String),
    /// Keyword list `=`: the list holds the value.
    Contains(Value),
}

/// A typed filter.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    And(Vec<Filter>),
    Or(Vec<Filter>),
    Not(Box<Filter>),
    Pred { field: Field, pred: Pred },
}

/// A typed query.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Resolved {
    pub filter: Option<Filter>,
    /// `(field, descending)`; empty means the default order.
    pub order_by: Vec<(Field, bool)>,
    /// Only `ExecutionStatus` is supported (as Temporal).
    pub group_by_status: bool,
}

/// Why a query is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error(transparent)]
    Parse(#[from] query::ParseError),
    #[error("invalid query: {0}")]
    Invalid(String),
}

fn invalid<T>(m: impl Into<String>) -> Result<T, QueryError> {
    Err(QueryError::Invalid(m.into()))
}

/// Parses a duration: an integer is nanoseconds (Temporal's encoding); a
/// string is Go-style (`1h30m`, `500ms`, `2.5s`). Answers milliseconds.
pub fn parse_duration_ms(l: &Lit) -> Result<i64, QueryError> {
    match l {
        Lit::Int(ns) => Ok(ns / 1_000_000),
        Lit::Float(ns) => Ok((ns / 1e6) as i64),
        Lit::Str(s) => {
            let mut total = 0f64;
            let mut rest = s.trim();
            if rest.is_empty() {
                return invalid(format!("{s:?} is not a duration"));
            }
            while !rest.is_empty() {
                let num_end = rest
                    .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                    .unwrap_or(rest.len());
                if num_end == 0 {
                    return invalid(format!("{s:?} is not a duration"));
                }
                let n: f64 = rest[..num_end]
                    .parse()
                    .map_err(|_| QueryError::Invalid(format!("{s:?} is not a duration")))?;
                rest = &rest[num_end..];
                let unit_end = rest
                    .find(|c: char| c.is_ascii_digit() || c == '.')
                    .unwrap_or(rest.len());
                let ms = match &rest[..unit_end] {
                    "ns" => 1e-6,
                    "us" | "µs" => 1e-3,
                    "ms" => 1.0,
                    "s" => 1e3,
                    "m" => 6e4,
                    "h" => 3.6e6,
                    "d" => 8.64e7,
                    _ => return invalid(format!("{s:?} is not a duration")),
                };
                total += n * ms;
                rest = &rest[unit_end..];
            }
            Ok(total as i64)
        }
        Lit::Bool(_) => invalid("a duration cannot be a boolean"),
    }
}

/// Parses a time: RFC 3339, or an integer of Unix nanoseconds.
pub fn parse_time_ms(l: &Lit) -> Result<i64, QueryError> {
    match l {
        Lit::Str(s) => DateTime::parse_from_rfc3339(s.trim())
            .map(|t| t.timestamp_millis())
            .map_err(|_| QueryError::Invalid(format!("{s:?} is not an RFC 3339 time"))),
        Lit::Int(ns) => Ok(ns.div_euclid(1_000_000)),
        _ => invalid(format!("{l} is not a time")),
    }
}

fn typed(field: &Field, l: &Lit) -> Result<Value, QueryError> {
    let name = || match field {
        Field::Column(c) => format!("{c:?}"),
        Field::Json { key, .. } => key.clone(),
    };
    if *field == Field::Column(Column::ExecutionStatus) {
        return match l {
            Lit::Str(s) => STATUSES
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(s))
                .map(|(_, v)| Value::Int(i64::from(*v)))
                .ok_or_else(|| QueryError::Invalid(format!("{s:?} is not an execution status"))),
            Lit::Int(i) if STATUSES.iter().any(|(_, v)| i64::from(*v) == *i) => Ok(Value::Int(*i)),
            other => invalid(format!("{other} is not an execution status")),
        };
    }
    if *field == Field::Column(Column::ExecutionDuration) {
        return parse_duration_ms(l).map(Value::TimeMs);
    }
    match (field.value_type(), l) {
        (ValueType::Keyword | ValueType::Text | ValueType::KeywordList, Lit::Str(s)) => {
            Ok(Value::Str(s.clone()))
        }
        (ValueType::Int, Lit::Int(i)) => Ok(Value::Int(*i)),
        (ValueType::Int, Lit::Str(s)) => s
            .trim()
            .parse()
            .map(Value::Int)
            .map_err(|_| QueryError::Invalid(format!("{} takes an integer, not {s:?}", name()))),
        (ValueType::Double, Lit::Int(i)) => Ok(Value::Float(*i as f64)),
        (ValueType::Double, Lit::Float(x)) => Ok(Value::Float(*x)),
        (ValueType::Double, Lit::Str(s)) => s
            .trim()
            .parse()
            .map(Value::Float)
            .map_err(|_| QueryError::Invalid(format!("{} takes a number, not {s:?}", name()))),
        (ValueType::Bool, Lit::Bool(b)) => Ok(Value::Bool(*b)),
        (ValueType::Bool, Lit::Str(s)) if s.eq_ignore_ascii_case("true") => Ok(Value::Bool(true)),
        (ValueType::Bool, Lit::Str(s)) if s.eq_ignore_ascii_case("false") => Ok(Value::Bool(false)),
        (ValueType::Datetime, l) => parse_time_ms(l).map(Value::TimeMs),
        (t, l) => invalid(format!("{} is {}; {l} does not fit", name(), t.name())),
    }
}

fn negate(f: Filter, negated: bool) -> Filter {
    if negated { Filter::Not(Box::new(f)) } else { f }
}

/// Resolves names against the system attributes and `custom`.
pub struct Resolver<'a> {
    pub custom: &'a BTreeMap<String, ValueType>,
}

impl Resolver<'_> {
    fn field(&self, name: &str) -> Result<Field, QueryError> {
        if let Some((_, c)) = COLUMNS.iter().find(|(n, _)| *n == name) {
            return Ok(Field::Column(*c));
        }
        if let Some((_, t)) = SYSTEM_JSON.iter().find(|(n, _)| *n == name) {
            return Ok(Field::Json {
                key: name.to_string(),
                ty: *t,
            });
        }
        if UNSUPPORTED.contains(&name) {
            return invalid(format!(
                "search attribute {name} is not supported by the Loams Temporal gateway yet"
            ));
        }
        match self.custom.get(name) {
            Some(t) => Ok(Field::Json {
                key: name.to_string(),
                ty: *t,
            }),
            None => invalid(format!("unknown search attribute {name}")),
        }
    }

    fn expr(&self, e: &Expr) -> Result<Filter, QueryError> {
        Ok(match e {
            Expr::And(a, b) => Filter::And(vec![self.expr(a)?, self.expr(b)?]),
            Expr::Or(a, b) => Filter::Or(vec![self.expr(a)?, self.expr(b)?]),
            Expr::Not(a) => Filter::Not(Box::new(self.expr(a)?)),
            Expr::Cmp { name, op, value } => {
                let field = self.field(name)?;
                let pred = match field.value_type() {
                    ValueType::Text => {
                        let Lit::Str(s) = value else {
                            return invalid(format!("{name} is Text; compare it with a string"));
                        };
                        match op {
                            CmpOp::Eq => Pred::TextMatch(s.clone()),
                            CmpOp::Ne => {
                                return Ok(Filter::Not(Box::new(Filter::Pred {
                                    field,
                                    pred: Pred::TextMatch(s.clone()),
                                })));
                            }
                            _ => return invalid(format!("{name} is Text: only = and != apply")),
                        }
                    }
                    ValueType::KeywordList => {
                        let v = typed(&field, value)?;
                        match op {
                            CmpOp::Eq => Pred::Contains(v),
                            CmpOp::Ne => {
                                return Ok(Filter::Not(Box::new(Filter::Pred {
                                    field,
                                    pred: Pred::Contains(v),
                                })));
                            }
                            _ => {
                                return invalid(format!(
                                    "{name} is a KeywordList: only =, != and IN apply"
                                ));
                            }
                        }
                    }
                    ValueType::Bool if !matches!(op, CmpOp::Eq | CmpOp::Ne) => {
                        return invalid(format!("{name} is Bool: only = and != apply"));
                    }
                    _ => Pred::Cmp(*op, typed(&field, value)?),
                };
                Filter::Pred { field, pred }
            }
            Expr::In {
                name,
                values,
                negated,
            } => {
                let field = self.field(name)?;
                if field.value_type() == ValueType::Text {
                    return invalid(format!("{name} is Text: IN does not apply"));
                }
                let vals = values
                    .iter()
                    .map(|v| typed(&field, v))
                    .collect::<Result<Vec<_>, _>>()?;
                let f = if field.value_type() == ValueType::KeywordList {
                    Filter::Or(
                        vals.into_iter()
                            .map(|v| Filter::Pred {
                                field: field.clone(),
                                pred: Pred::Contains(v),
                            })
                            .collect(),
                    )
                } else {
                    Filter::Pred {
                        field,
                        pred: Pred::In(vals),
                    }
                };
                negate(f, *negated)
            }
            Expr::Between {
                name,
                low,
                high,
                negated,
            } => {
                let field = self.field(name)?;
                if matches!(
                    field.value_type(),
                    ValueType::Text | ValueType::KeywordList | ValueType::Bool
                ) {
                    return invalid(format!("BETWEEN does not apply to {name}"));
                }
                let pred = Pred::Between(typed(&field, low)?, typed(&field, high)?);
                negate(Filter::Pred { field, pred }, *negated)
            }
            Expr::StartsWith {
                name,
                prefix,
                negated,
            } => {
                let field = self.field(name)?;
                if field.value_type() != ValueType::Keyword
                    || field == Field::Column(Column::ExecutionStatus)
                {
                    return invalid(format!(
                        "STARTS_WITH applies to Keyword attributes, not {name}"
                    ));
                }
                let Lit::Str(p) = prefix else {
                    return invalid("STARTS_WITH takes a string");
                };
                negate(
                    Filter::Pred {
                        field,
                        pred: Pred::StartsWith(p.clone()),
                    },
                    *negated,
                )
            }
            Expr::IsNull { name, negated } => {
                let field = self.field(name)?;
                negate(
                    Filter::Pred {
                        field,
                        pred: Pred::IsNull,
                    },
                    *negated,
                )
            }
        })
    }

    /// Types a parsed query.
    pub fn resolve(&self, q: &Query) -> Result<Resolved, QueryError> {
        let filter = q.filter.as_ref().map(|e| self.expr(e)).transpose()?;
        let mut order_by = Vec::new();
        for o in &q.order_by {
            let f = self.field(&o.name)?;
            if matches!(f.value_type(), ValueType::Text | ValueType::KeywordList) {
                return invalid(format!("cannot ORDER BY {}", o.name));
            }
            order_by.push((f, o.descending));
        }
        let group_by_status = match q.group_by.as_slice() {
            [] => false,
            [g] if g == "ExecutionStatus" => true,
            _ => return invalid("only GROUP BY ExecutionStatus is supported"),
        };
        Ok(Resolved {
            filter,
            order_by,
            group_by_status,
        })
    }
}

/// Parses and types `query` with `custom` attributes.
pub fn compile(query: &str, custom: &BTreeMap<String, ValueType>) -> Result<Resolved, QueryError> {
    let q = query::parse(query)?;
    Resolver { custom }.resolve(&q)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom() -> BTreeMap<String, ValueType> {
        [
            ("CustomerId", ValueType::Keyword),
            ("Amount", ValueType::Double),
            ("Retries", ValueType::Int),
            ("Urgent", ValueType::Bool),
            ("Notes", ValueType::Text),
            ("Tags", ValueType::KeywordList),
            ("DueAt", ValueType::Datetime),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    fn one(q: &str) -> Filter {
        compile(q, &custom()).unwrap().filter.unwrap()
    }

    #[test]
    fn statuses_and_times_are_typed() {
        assert_eq!(
            one("ExecutionStatus = 'running'"),
            Filter::Pred {
                field: Field::Column(Column::ExecutionStatus),
                pred: Pred::Cmp(CmpOp::Eq, Value::Int(1))
            }
        );
        assert_eq!(
            one("StartTime > '2026-10-27T00:00:00Z'"),
            Filter::Pred {
                field: Field::Column(Column::StartTime),
                pred: Pred::Cmp(CmpOp::Gt, Value::TimeMs(1_793_059_200_000))
            }
        );
        assert_eq!(
            one("CloseTime < 1793059200000000000"),
            Filter::Pred {
                field: Field::Column(Column::CloseTime),
                pred: Pred::Cmp(CmpOp::Lt, Value::TimeMs(1_793_059_200_000))
            }
        );
        assert_eq!(
            one("ExecutionDuration > '1h30m'"),
            Filter::Pred {
                field: Field::Column(Column::ExecutionDuration),
                pred: Pred::Cmp(CmpOp::Gt, Value::TimeMs(5_400_000))
            }
        );
    }

    #[test]
    fn custom_types() {
        assert!(matches!(
            one("Amount >= 10"),
            Filter::Pred { pred: Pred::Cmp(CmpOp::Ge, Value::Float(x)), .. } if x == 10.0
        ));
        assert!(matches!(
            one("Urgent = 'true'"),
            Filter::Pred {
                pred: Pred::Cmp(CmpOp::Eq, Value::Bool(true)),
                ..
            }
        ));
        assert!(matches!(
            one("Notes = 'late delivery'"),
            Filter::Pred {
                pred: Pred::TextMatch(_),
                ..
            }
        ));
        assert!(matches!(one("Notes != 'x'"), Filter::Not(_)));
        assert!(matches!(
            one("Tags = 'vip'"),
            Filter::Pred {
                pred: Pred::Contains(_),
                ..
            }
        ));
        assert!(matches!(one("Tags IN ('a', 'b')"), Filter::Or(v) if v.len() == 2));
        assert!(matches!(one("CustomerId NOT IN ('a')"), Filter::Not(_)));
        assert!(matches!(
            one("Retries BETWEEN 1 AND '3'"),
            Filter::Pred {
                pred: Pred::Between(Value::Int(1), Value::Int(3)),
                ..
            }
        ));
        assert!(matches!(
            one("TemporalScheduledById = 's'"),
            Filter::Pred {
                field: Field::Json { .. },
                ..
            }
        ));
    }

    #[test]
    fn refusals() {
        for bad in [
            "Nope = 1",
            "HistoryLength > 10",
            "ExecutionStatus = 'Sleeping'",
            "StartTime > 'yesterday'",
            "Retries = 'many'",
            "Notes > 'a'",
            "Notes IN ('a')",
            "Tags > 'a'",
            "Urgent < true",
            "Amount STARTS_WITH 'x'",
            "ExecutionStatus STARTS_WITH 'R'",
            "Urgent BETWEEN true AND false",
            "Amount = true",
            "WorkflowId = 1",
            "ORDER BY Notes",
            "GROUP BY WorkflowType",
        ] {
            assert!(compile(bad, &custom()).is_err(), "{bad:?} compiled");
        }
    }

    #[test]
    fn order_and_group() {
        let r = compile(
            "ORDER BY StartTime DESC GROUP BY ExecutionStatus",
            &custom(),
        )
        .unwrap();
        assert_eq!(r.order_by, vec![(Field::Column(Column::StartTime), true)]);
        assert!(r.group_by_status);
    }

    #[test]
    fn durations_and_names() {
        assert_eq!(parse_duration_ms(&Lit::Str("500ms".into())).unwrap(), 500);
        assert_eq!(parse_duration_ms(&Lit::Str("2.5s".into())).unwrap(), 2500);
        assert_eq!(parse_duration_ms(&Lit::Int(3_000_000_000)).unwrap(), 3000);
        assert!(parse_duration_ms(&Lit::Str("5 parsecs".into())).is_err());
        assert!(parse_duration_ms(&Lit::Str("h".into())).is_err());
        assert_eq!(status_name(7), "TimedOut");
        assert!(is_system("WorkflowId") && is_system("HistoryLength") && !is_system("CustomerId"));
        for t in 1..=7 {
            assert_eq!(ValueType::from_i32(t).unwrap().to_i32(), t);
        }
        assert!(ValueType::from_i32(0).is_none());
        assert_eq!(
            system_attributes().get("StartTime"),
            Some(&ValueType::Datetime)
        );
    }
}
