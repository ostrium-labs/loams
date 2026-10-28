//! A typed filter as SQL on the `visibility` table (D2074): a `WHERE`
//! fragment with numbered parameters, never values spliced into the text.
//! Search-attribute keys are parameters too.
//!
//! Three-valued logic: a predicate on a missing attribute (or a null close
//! time) is unknown, which `WHERE` treats as false; under `NOT` it is
//! coalesced to false first, so `NOT x` keeps the rows without the
//! attribute, as the in-memory evaluator ([`crate::eval`]) does.

use crate::filter::{Column, Field, Filter, Pred, Resolved, Value, ValueType};
use crate::query::CmpOp;

/// A bind parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// SQL built so far: parameters are numbered from `base + 1`.
#[derive(Debug, Clone, PartialEq)]
pub struct Builder {
    base: usize,
    pub params: Vec<Param>,
}

impl Builder {
    /// Parameters start at `$<base + 1>` (the caller binds `$1..$base`).
    pub fn new(base: usize) -> Self {
        Builder {
            base,
            params: Vec::new(),
        }
    }

    fn param(&mut self, p: Param) -> String {
        self.params.push(p);
        format!("${}", self.base + self.params.len())
    }

    fn column(c: Column) -> &'static str {
        match c {
            Column::WorkflowId => "workflow_id",
            Column::RunId => "run_id",
            Column::WorkflowType => "workflow_type",
            Column::ExecutionStatus => "status",
            Column::TaskQueue => "task_queue",
            Column::StartTime => "start_time",
            Column::CloseTime => "close_time",
            Column::ExecutionTime => "execution_time",
            Column::ExecutionDuration => "(extract(epoch FROM (close_time - start_time)) * 1000)",
        }
    }

    /// The SQL expression of a field, typed for comparison.
    fn field(&mut self, f: &Field) -> String {
        match f {
            Field::Column(c) => Self::column(*c).to_string(),
            Field::Json { key, ty } => {
                let k = self.param(Param::Text(key.clone()));
                match ty {
                    ValueType::Keyword | ValueType::Text => format!("(search_attrs->>{k})"),
                    ValueType::Int => format!("((search_attrs->>{k})::numeric)"),
                    ValueType::Double => format!("((search_attrs->>{k})::double precision)"),
                    ValueType::Bool => format!("((search_attrs->>{k})::boolean)"),
                    ValueType::Datetime => format!("((search_attrs->>{k})::timestamptz)"),
                    ValueType::KeywordList => format!("(search_attrs->{k})"),
                }
            }
        }
    }

    /// The SQL of a value compared with `f`.
    fn value(&mut self, f: &Field, v: &Value) -> String {
        match v {
            Value::Str(s) => {
                let p = self.param(Param::Text(s.clone()));
                format!("{p}::text")
            }
            Value::Int(i) => {
                let p = self.param(Param::Int(*i));
                format!("{p}::bigint")
            }
            Value::Float(x) => {
                let p = self.param(Param::Float(*x));
                format!("{p}::double precision")
            }
            Value::Bool(b) => {
                let p = self.param(Param::Bool(*b));
                format!("{p}::boolean")
            }
            Value::TimeMs(ms) => {
                let p = self.param(Param::Int(*ms));
                if *f == Field::Column(Column::ExecutionDuration) {
                    format!("{p}::double precision")
                } else {
                    format!("to_timestamp({p}::bigint / 1000.0)")
                }
            }
        }
    }

    fn pred(&mut self, field: &Field, pred: &Pred) -> String {
        // Keyword equality on a JSON key is containment, which the GIN
        // index on search_attrs serves.
        if let (
            Field::Json {
                key,
                ty: ValueType::Keyword,
            },
            Pred::Cmp(CmpOp::Eq, Value::Str(s)),
        ) = (field, pred)
        {
            let k = self.param(Param::Text(key.clone()));
            let v = self.param(Param::Text(s.clone()));
            return format!("(search_attrs @> jsonb_build_object({k}::text, {v}::text))");
        }
        match pred {
            Pred::Cmp(op, v) => {
                let x = self.field(field);
                let v = self.value(field, v);
                format!("({x} {} {v})", op.sql())
            }
            Pred::In(vs) => {
                let x = self.field(field);
                let list: Vec<String> = vs.iter().map(|v| self.value(field, v)).collect();
                format!("({x} IN ({}))", list.join(", "))
            }
            Pred::Between(lo, hi) => {
                let x = self.field(field);
                let lo = self.value(field, lo);
                let hi = self.value(field, hi);
                format!("({x} BETWEEN {lo} AND {hi})")
            }
            Pred::StartsWith(prefix) => {
                let x = self.field(field);
                let p = self.param(Param::Text(prefix.clone()));
                format!("starts_with({x}, {p}::text)")
            }
            Pred::IsNull => match field {
                Field::Column(c) => format!("({} IS NULL)", Self::column(*c)),
                Field::Json { key, .. } => {
                    let k = self.param(Param::Text(key.clone()));
                    format!("(NOT (search_attrs ? {k}::text))")
                }
            },
            Pred::TextMatch(text) => {
                let x = self.field(field);
                let p = self.param(Param::Text(text.clone()));
                format!(
                    "(to_tsvector('simple', coalesce({x}, '')) @@ plainto_tsquery('simple', {p}::text))"
                )
            }
            Pred::Contains(v) => {
                let x = self.field(field);
                let s = match v {
                    Value::Str(s) => s.clone(),
                    other => format!("{other:?}"),
                };
                let p = self.param(Param::Text(s));
                format!("({x} @> to_jsonb({p}::text))")
            }
        }
    }

    /// The `WHERE` fragment of a filter.
    pub fn filter(&mut self, f: &Filter) -> String {
        match f {
            Filter::And(fs) if fs.is_empty() => "TRUE".into(),
            Filter::Or(fs) if fs.is_empty() => "FALSE".into(),
            Filter::And(fs) => {
                let parts: Vec<String> = fs.iter().map(|f| self.filter(f)).collect();
                format!("({})", parts.join(" AND "))
            }
            Filter::Or(fs) => {
                let parts: Vec<String> = fs.iter().map(|f| self.filter(f)).collect();
                format!("({})", parts.join(" OR "))
            }
            Filter::Not(f) => {
                let inner = self.filter(f);
                format!("(NOT coalesce({inner}, FALSE))")
            }
            Filter::Pred { field, pred } => self.pred(field, pred),
        }
    }

    /// The `ORDER BY` list: the query's, then the run ID; or Temporal's
    /// default (open runs first, then by close time, then start time).
    pub fn order_by(&mut self, r: &Resolved) -> String {
        if r.order_by.is_empty() {
            return "coalesce(close_time, 'infinity'::timestamptz) DESC, start_time DESC, run_id"
                .into();
        }
        let mut parts: Vec<String> = r
            .order_by
            .iter()
            .map(|(f, desc)| {
                let x = self.field(f);
                format!("{x} {} NULLS LAST", if *desc { "DESC" } else { "ASC" })
            })
            .collect();
        parts.push("run_id".into());
        parts.join(", ")
    }
}

/// The `WHERE` clause (after `namespace_id = $1`) and its parameters, from
/// `$<base + 1>`.
pub fn where_clause(r: &Resolved, base: usize) -> (String, Vec<Param>) {
    let mut b = Builder::new(base);
    let w = match &r.filter {
        Some(f) => b.filter(f),
        None => "TRUE".into(),
    };
    (w, b.params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::compile;
    use std::collections::BTreeMap;

    fn sql(q: &str) -> (String, Vec<Param>) {
        let custom: BTreeMap<String, ValueType> = [
            ("CustomerId", ValueType::Keyword),
            ("Amount", ValueType::Double),
            ("Notes", ValueType::Text),
            ("Tags", ValueType::KeywordList),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        where_clause(&compile(q, &custom).unwrap(), 1)
    }

    #[test]
    fn columns_with_numbered_params() {
        let (w, p) = sql("WorkflowType = 'order' AND ExecutionStatus IN ('Running', 'Failed')");
        assert_eq!(
            w,
            "((workflow_type = $2::text) AND (status IN ($3::bigint, $4::bigint)))"
        );
        assert_eq!(
            p,
            vec![Param::Text("order".into()), Param::Int(1), Param::Int(3)]
        );
    }

    #[test]
    fn times_durations_and_nulls() {
        let (w, p) = sql(
            "StartTime BETWEEN '2026-10-27T00:00:00Z' AND '2026-10-28T00:00:00Z' OR CloseTime IS NULL",
        );
        assert_eq!(
            w,
            "((start_time BETWEEN to_timestamp($2::bigint / 1000.0) AND to_timestamp($3::bigint / 1000.0)) OR (close_time IS NULL))"
        );
        assert_eq!(
            p,
            vec![Param::Int(1_793_059_200_000), Param::Int(1_793_145_600_000)]
        );
        let (w, _) = sql("ExecutionDuration > '1m'");
        assert_eq!(
            w,
            "((extract(epoch FROM (close_time - start_time)) * 1000) > $2::double precision)"
        );
    }

    #[test]
    fn json_attributes() {
        let (w, p) = sql("CustomerId = 'c-1'");
        assert_eq!(
            w,
            "(search_attrs @> jsonb_build_object($2::text, $3::text))"
        );
        assert_eq!(
            p,
            vec![Param::Text("CustomerId".into()), Param::Text("c-1".into())]
        );
        let (w, _) = sql("Amount > 10");
        assert_eq!(
            w,
            "(((search_attrs->>$2)::double precision) > $3::double precision)"
        );
        let (w, _) = sql("Tags = 'vip'");
        assert_eq!(w, "((search_attrs->$2) @> to_jsonb($3::text))");
        let (w, _) = sql("Notes = 'late box'");
        assert!(w.contains("plainto_tsquery"), "{w}");
        let (w, _) = sql("CustomerId IS NULL");
        assert_eq!(w, "(NOT (search_attrs ? $2::text))");
        let (w, _) = sql("CustomerId STARTS_WITH 'c-'");
        assert_eq!(w, "starts_with((search_attrs->>$2), $3::text)");
    }

    #[test]
    fn negation_coalesces_unknown() {
        let (w, _) = sql("NOT CustomerId = 'x'");
        assert_eq!(
            w,
            "(NOT coalesce((search_attrs @> jsonb_build_object($2::text, $3::text)), FALSE))"
        );
        let (w, _) = sql("WorkflowId NOT IN ('a')");
        assert_eq!(w, "(NOT coalesce((workflow_id IN ($2::text)), FALSE))");
    }

    #[test]
    fn no_value_reaches_the_text() {
        let (w, p) = sql("CustomerId = 'x''); DROP TABLE visibility; --'");
        assert!(!w.contains("DROP"), "{w}");
        assert_eq!(p[1], Param::Text("x'); DROP TABLE visibility; --".into()));
    }

    #[test]
    fn ordering() {
        let r = compile("ORDER BY StartTime DESC", &BTreeMap::new()).unwrap();
        assert_eq!(
            Builder::new(1).order_by(&r),
            "start_time DESC NULLS LAST, run_id"
        );
        let r = compile("", &BTreeMap::new()).unwrap();
        assert!(
            Builder::new(1)
                .order_by(&r)
                .starts_with("coalesce(close_time")
        );
        assert_eq!(where_clause(&r, 1).0, "TRUE");
    }
}
