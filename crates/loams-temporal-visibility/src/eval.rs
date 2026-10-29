//! The typed filter evaluated in memory (the dev store, and ListSchedules'
//! queries), with the same semantics as [`crate::sql`]: a predicate on a
//! missing value is false, and `NOT` of it is true.

use std::cmp::Ordering;

use chrono::DateTime;

use crate::filter::{Column, Field, Filter, Pred, Resolved, Value, ValueType};
use crate::query::CmpOp;

/// One run's visibility record. Times are Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Record {
    pub workflow_id: String,
    pub run_id: String,
    pub workflow_type: String,
    pub status: i32,
    pub task_queue: String,
    pub start_ms: i64,
    pub close_ms: Option<i64>,
    pub execution_ms: i64,
    /// Search attributes, decoded (JSON values by key).
    pub search_attrs: serde_json::Map<String, serde_json::Value>,
    /// The encoded `Memo`, opaque.
    pub memo: Option<Vec<u8>>,
}

/// A value read from a record.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Str(String),
    Num(f64),
    Bool(bool),
    List(Vec<String>),
}

fn time_cell(ms: i64) -> Cell {
    Cell::Num(ms as f64)
}

fn json_cell(v: &serde_json::Value, ty: ValueType) -> Option<Cell> {
    match ty {
        ValueType::Keyword | ValueType::Text => v.as_str().map(|s| Cell::Str(s.to_string())),
        ValueType::Int | ValueType::Double => v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            .map(Cell::Num),
        ValueType::Bool => v.as_bool().map(Cell::Bool),
        ValueType::Datetime => v
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| time_cell(t.timestamp_millis())),
        ValueType::KeywordList => match v {
            serde_json::Value::Array(a) => Some(Cell::List(
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect(),
            )),
            serde_json::Value::String(s) => Some(Cell::List(vec![s.clone()])),
            _ => None,
        },
    }
}

fn cell(r: &Record, f: &Field) -> Option<Cell> {
    match f {
        Field::Column(c) => match c {
            Column::WorkflowId => Some(Cell::Str(r.workflow_id.clone())),
            Column::RunId => Some(Cell::Str(r.run_id.clone())),
            Column::WorkflowType => Some(Cell::Str(r.workflow_type.clone())),
            Column::ExecutionStatus => Some(Cell::Num(f64::from(r.status))),
            Column::TaskQueue => Some(Cell::Str(r.task_queue.clone())),
            Column::StartTime => Some(time_cell(r.start_ms)),
            Column::CloseTime => r.close_ms.map(time_cell),
            Column::ExecutionTime => Some(time_cell(r.execution_ms)),
            Column::ExecutionDuration => r.close_ms.map(|c| time_cell(c - r.start_ms)),
        },
        Field::Json { key, ty } => r.search_attrs.get(key).and_then(|v| json_cell(v, *ty)),
    }
}

fn value_cell(v: &Value) -> Cell {
    match v {
        Value::Str(s) => Cell::Str(s.clone()),
        Value::Int(i) => Cell::Num(*i as f64),
        Value::Float(x) => Cell::Num(*x),
        Value::Bool(b) => Cell::Bool(*b),
        Value::TimeMs(ms) => time_cell(*ms),
    }
}

fn compare(a: &Cell, b: &Cell) -> Option<Ordering> {
    match (a, b) {
        (Cell::Str(x), Cell::Str(y)) => Some(x.cmp(y)),
        (Cell::Num(x), Cell::Num(y)) => x.partial_cmp(y),
        (Cell::Bool(x), Cell::Bool(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

fn cmp_holds(op: CmpOp, o: Ordering) -> bool {
    match op {
        CmpOp::Eq => o == Ordering::Equal,
        CmpOp::Ne => o != Ordering::Equal,
        CmpOp::Lt => o == Ordering::Less,
        CmpOp::Le => o != Ordering::Greater,
        CmpOp::Gt => o == Ordering::Greater,
        CmpOp::Ge => o != Ordering::Less,
    }
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn pred_holds(r: &Record, field: &Field, pred: &Pred) -> bool {
    if let Pred::IsNull = pred {
        return match field {
            Field::Json { key, .. } => !r.search_attrs.contains_key(key),
            _ => cell(r, field).is_none(),
        };
    }
    let Some(c) = cell(r, field) else {
        return false;
    };
    match pred {
        Pred::Cmp(op, v) => compare(&c, &value_cell(v)).is_some_and(|o| cmp_holds(*op, o)),
        Pred::In(vs) => vs
            .iter()
            .any(|v| compare(&c, &value_cell(v)) == Some(Ordering::Equal)),
        Pred::Between(lo, hi) => {
            compare(&c, &value_cell(lo)).is_some_and(|o| o != Ordering::Less)
                && compare(&c, &value_cell(hi)).is_some_and(|o| o != Ordering::Greater)
        }
        Pred::StartsWith(p) => matches!(&c, Cell::Str(s) if s.starts_with(p.as_str())),
        Pred::TextMatch(t) => match &c {
            Cell::Str(s) => {
                let have = words(s);
                words(t).iter().all(|w| have.contains(w))
            }
            _ => false,
        },
        Pred::Contains(v) => match (&c, v) {
            (Cell::List(l), Value::Str(s)) => l.contains(s),
            _ => false,
        },
        Pred::IsNull => false,
    }
}

/// Whether `r` passes `f`.
pub fn matches(f: &Filter, r: &Record) -> bool {
    match f {
        Filter::And(fs) => fs.iter().all(|f| matches(f, r)),
        Filter::Or(fs) => fs.iter().any(|f| matches(f, r)),
        Filter::Not(f) => !matches(f, r),
        Filter::Pred { field, pred } => pred_holds(r, field, pred),
    }
}

/// Whether `r` passes the query's filter (if any).
pub fn passes(q: &Resolved, r: &Record) -> bool {
    q.filter.as_ref().is_none_or(|f| matches(f, r))
}

/// Sorts records as the SQL store does: the query's order (missing values
/// last), then the run ID; or open runs first, then by close time and start
/// time, newest first.
pub fn sort(q: &Resolved, rs: &mut [Record]) {
    rs.sort_by(|a, b| {
        if q.order_by.is_empty() {
            let close = |r: &Record| r.close_ms.unwrap_or(i64::MAX);
            return close(b)
                .cmp(&close(a))
                .then(b.start_ms.cmp(&a.start_ms))
                .then(a.run_id.cmp(&b.run_id));
        }
        for (f, desc) in &q.order_by {
            let o = match (cell(a, f), cell(b, f)) {
                (Some(x), Some(y)) => {
                    let o = compare(&x, &y).unwrap_or(Ordering::Equal);
                    if *desc { o.reverse() } else { o }
                }
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        a.run_id.cmp(&b.run_id)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::compile;
    use std::collections::BTreeMap;

    fn custom() -> BTreeMap<String, ValueType> {
        [
            ("CustomerId", ValueType::Keyword),
            ("Amount", ValueType::Double),
            ("Notes", ValueType::Text),
            ("Tags", ValueType::KeywordList),
            ("Urgent", ValueType::Bool),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    fn rec(
        id: &str,
        status: i32,
        start: i64,
        close: Option<i64>,
        attrs: serde_json::Value,
    ) -> Record {
        Record {
            workflow_id: id.into(),
            run_id: format!("run-{id}"),
            workflow_type: if id.starts_with('p') {
                "payment".into()
            } else {
                "order".into()
            },
            status,
            task_queue: "q".into(),
            start_ms: start,
            close_ms: close,
            execution_ms: start,
            search_attrs: attrs.as_object().cloned().unwrap_or_default(),
            memo: None,
        }
    }

    fn data() -> Vec<Record> {
        vec![
            rec(
                "o1",
                1,
                1_000,
                None,
                serde_json::json!({"CustomerId": "c1", "Amount": 5, "Tags": ["vip", "eu"]}),
            ),
            rec(
                "o2",
                2,
                2_000,
                Some(62_000),
                serde_json::json!({"CustomerId": "c2", "Amount": 50.5, "Notes": "Late delivery, box damaged"}),
            ),
            rec(
                "p1",
                3,
                3_000,
                Some(4_000),
                serde_json::json!({"Urgent": true, "Tags": "eu"}),
            ),
            rec("p2", 1, 4_000, None, serde_json::json!({})),
        ]
    }

    fn ids(q: &str) -> Vec<String> {
        let r = compile(q, &custom()).unwrap();
        let mut rs: Vec<Record> = data().into_iter().filter(|x| passes(&r, x)).collect();
        sort(&r, &mut rs);
        rs.into_iter().map(|r| r.workflow_id).collect()
    }

    #[test]
    fn filters() {
        assert_eq!(ids("ExecutionStatus = 'Running'"), ["p2", "o1"]);
        assert_eq!(
            ids("WorkflowType STARTS_WITH 'pay' ORDER BY StartTime"),
            ["p1", "p2"]
        );
        assert_eq!(ids("Amount > 10"), ["o2"]);
        assert_eq!(ids("Tags = 'eu' ORDER BY WorkflowId"), ["o1", "p1"]);
        assert_eq!(ids("Notes = 'box late'"), ["o2"]);
        assert_eq!(ids("Notes = 'box early'"), Vec::<String>::new());
        assert_eq!(ids("CustomerId IS NULL ORDER BY WorkflowId"), ["p1", "p2"]);
        assert_eq!(
            ids("NOT CustomerId = 'c1' ORDER BY WorkflowId"),
            ["o2", "p1", "p2"]
        );
        assert_eq!(
            ids("CloseTime IS NOT NULL AND ExecutionDuration >= '1m'"),
            ["o2"]
        );
        assert_eq!(ids("Urgent = true OR WorkflowId IN ('o1')"), ["o1", "p1"]);
        assert_eq!(
            ids("StartTime BETWEEN 2000000000 AND 3000000000 ORDER BY StartTime DESC"),
            ["p1", "o2"]
        );
    }

    #[test]
    fn default_order_is_open_first_then_newest() {
        assert_eq!(ids(""), ["p2", "o1", "o2", "p1"]);
    }

    #[test]
    fn order_puts_missing_values_last() {
        assert_eq!(ids("ORDER BY Amount DESC"), ["o2", "o1", "p1", "p2"]);
    }
}
