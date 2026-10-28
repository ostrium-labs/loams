//! ClickHouse's output column names (plan HE1 Task 22, step 4).
//!
//! A select item without an alias is named by its expression as ClickHouse
//! prints it, with every operator written as its function: `1 + 2` is
//! `plus(1, 2)`, `x IN (1, 2)` is `in(x, (1, 2))`, `CASE WHEN …` is
//! `multiIf(…)`. Clients read results by these names, so they are part of the
//! answer. The rules were measured on the 26.9.2.8 oracle;
//! `tests/fixtures/names.tsv` keeps its answers.
//!
//! A subquery is named `_subquery_N` (R25.4): `N` counts the statement's
//! subqueries in the order ClickHouse's analyzer meets them ([`subquery_numbers`]).

use datafusion::sql::sqlparser::ast::{
    AccessExpr, BinaryOperator, CastKind, DateTimeField, Expr, Function, FunctionArg,
    FunctionArgExpr, FunctionArguments, Ident, LambdaFunction, OneOrManyWithParens, Subscript,
    UnaryOperator, Value, WindowType,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use datafusion::sql::sqlparser::ast::{
    GroupByExpr, Query, Select, SelectItem, SetExpr, TableFactor, TableWithJoins,
};
use loams_house_types::ty::quote;

use crate::scope::{Walk, children};

/// The name ClickHouse gives a select item `e` that has no alias.
pub fn column_name(e: &Expr) -> String {
    let mut out = String::new();
    write(e, &mut out);
    out
}

/// A statement's subqueries, by the position of their first `SELECT`, to the
/// `N` of `_subquery_N`.
pub type Numbers = HashMap<(u64, u64), usize>;

thread_local! {
    static NUMBERS: RefCell<Option<Numbers>> = const { RefCell::new(None) };
}

/// [`column_name`], subqueries named `_subquery_N` by `numbers`.
pub fn column_name_numbered(e: &Expr, numbers: &Numbers) -> String {
    NUMBERS.with(|n| *n.borrow_mut() = Some(numbers.clone()));
    let name = column_name(e);
    NUMBERS.with(|n| *n.borrow_mut() = None);
    name
}

/// Where a query's first `SELECT` is.
fn query_key(q: &Query) -> Option<(u64, u64)> {
    fn body(s: &SetExpr) -> Option<(u64, u64)> {
        match s {
            SetExpr::Select(sel) => {
                let at = sel.select_token.0.span.start;
                Some((at.line, at.column))
            }
            SetExpr::Query(q) => body(&q.body),
            SetExpr::SetOperation { left, .. } => body(left),
            _ => None,
        }
    }
    body(&q.body)
}

fn subquery_name(q: &Query) -> Option<String> {
    let key = query_key(q)?;
    NUMBERS.with(|n| {
        n.borrow()
            .as_ref()
            .and_then(|m| m.get(&key))
            .map(|n| format!("_subquery_{n}"))
    })
}

/// Numbers a statement's subqueries as ClickHouse's analyzer does, measured
/// on the oracle: one counter for the statement; scalar `WITH` first, then
/// the `FROM` tree (a derived table, or a reference to a CTE, counts), then
/// the select list, `WHERE`, `GROUP BY`, `HAVING` and `ORDER BY`; a
/// subquery is counted before the subqueries inside it, and `EXISTS` counts
/// two.
pub fn subquery_numbers(q: &Query) -> Numbers {
    let mut n = Numberer {
        next: 1,
        map: Numbers::new(),
        ctes: HashSet::new(),
    };
    let mut q = q.clone();
    n.query_body(&mut q);
    n.map
}

struct Numberer {
    next: usize,
    map: Numbers,
    ctes: HashSet<String>,
}

impl Numberer {
    fn number(&mut self, q: &Query) {
        if let Some(k) = query_key(q) {
            self.map.entry(k).or_insert(self.next);
        }
        self.next += 1;
    }

    fn query_body(&mut self, q: &mut Query) {
        if let Some(w) = &mut q.with {
            for cte in &mut w.cte_tables {
                match crate::scope::scalar_with_expr(&mut cte.query) {
                    Some(e) => {
                        let _ = self.expr(e);
                    }
                    None => {
                        self.ctes.insert(cte.alias.name.value.clone());
                    }
                }
            }
        }
        self.set_expr(&mut q.body);
        if let Some(ob) = &mut q.order_by
            && let datafusion::sql::sqlparser::ast::OrderByKind::Expressions(items) = &mut ob.kind
        {
            for o in items {
                let _ = self.expr(&mut o.expr);
            }
        }
    }

    fn set_expr(&mut self, s: &mut SetExpr) {
        match s {
            SetExpr::Select(sel) => self.select(sel),
            SetExpr::Query(q) => self.query_body(q),
            SetExpr::SetOperation { left, right, .. } => {
                self.set_expr(left);
                self.set_expr(right);
            }
            _ => {}
        }
    }

    fn table_factor(&mut self, t: &mut TableFactor) {
        match t {
            TableFactor::Derived { subquery, .. } => {
                self.number(subquery);
                self.query_body(subquery);
            }
            TableFactor::Table { name, .. } => {
                if name.0.len() == 1 && self.ctes.contains(&name.to_string()) {
                    self.next += 1;
                }
            }
            TableFactor::NestedJoin {
                table_with_joins, ..
            } => self.from(table_with_joins),
            _ => {}
        }
    }

    fn from(&mut self, t: &mut TableWithJoins) {
        self.table_factor(&mut t.relation);
        for j in &mut t.joins {
            self.table_factor(&mut j.relation);
        }
    }

    fn select(&mut self, s: &mut Select) {
        for t in &mut s.from {
            self.from(t);
        }
        for item in &mut s.projection {
            if let SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } = item {
                let _ = self.expr(e);
            }
        }
        for e in s.prewhere.iter_mut().chain(s.selection.iter_mut()) {
            let _ = self.expr(e);
        }
        if let GroupByExpr::Expressions(exprs, _) = &mut s.group_by {
            for e in exprs {
                let _ = self.expr(e);
            }
        }
        if let Some(h) = &mut s.having {
            let _ = self.expr(h);
        }
    }
}

impl Walk for Numberer {
    fn expr(&mut self, e: &mut Expr) -> Result<(), loams_house_types::ChError> {
        match e {
            Expr::Exists { subquery, .. } => {
                self.number(subquery);
                self.next += 1;
                self.query_body(subquery);
                Ok(())
            }
            _ => children(e, self),
        }
    }

    fn query(&mut self, q: &mut Query) -> Result<(), loams_house_types::ChError> {
        self.number(q);
        self.query_body(q);
        Ok(())
    }
}

fn call(out: &mut String, name: &str, args: &[&Expr]) {
    out.push_str(name);
    out.push('(');
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write(a, out);
    }
    out.push(')');
}

/// The function a binary operator is in ClickHouse.
pub fn operator_function(op: &BinaryOperator) -> Option<&'static str> {
    Some(match op {
        BinaryOperator::Plus => "plus",
        BinaryOperator::Minus => "minus",
        BinaryOperator::Multiply => "multiply",
        BinaryOperator::Divide => "divide",
        BinaryOperator::Modulo => "modulo",
        BinaryOperator::Eq => "equals",
        BinaryOperator::NotEq => "notEquals",
        BinaryOperator::Lt => "less",
        BinaryOperator::LtEq => "lessOrEquals",
        BinaryOperator::Gt => "greater",
        BinaryOperator::GtEq => "greaterOrEquals",
        BinaryOperator::And => "and",
        BinaryOperator::Or => "or",
        BinaryOperator::StringConcat => "concat",
        BinaryOperator::DuckIntegerDivide | BinaryOperator::MyIntegerDivide => "intDiv",
        _ => return None,
    })
}

/// `and(a, b, c)` for `a AND b AND c`: ClickHouse flattens chains.
fn flatten<'a>(e: &'a Expr, op: &BinaryOperator, out: &mut Vec<&'a Expr>) {
    match e {
        Expr::BinaryOp { left, op: o, right } if o == op => {
            flatten(left, op, out);
            flatten(right, op, out);
        }
        other => out.push(other),
    }
}

fn ident(i: &Ident, out: &mut String) {
    out.push_str(&i.value);
}

/// A numeric literal as ClickHouse prints it: integers as they are, floats in
/// shortest form with a trailing `.` when integral (`1e3` is `1000.`).
pub fn number_text(n: &str) -> String {
    let plain = n.trim_start_matches('+');
    if plain.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return plain.to_string();
    }
    match plain.parse::<f64>() {
        Ok(f) => {
            let text = loams_house_types::text::float64(f);
            if f.is_finite() && !text.contains(['.', 'e']) {
                format!("{text}.")
            } else {
                text
            }
        }
        Err(_) => plain.to_string(),
    }
}

fn value(v: &Value, out: &mut String) {
    match v {
        Value::Number(n, _) => out.push_str(&number_text(n)),
        Value::SingleQuotedString(s)
        | Value::DoubleQuotedString(s)
        | Value::EscapedStringLiteral(s) => out.push_str(&quote(s)),
        Value::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Null => out.push_str("NULL"),
        other => out.push_str(&other.to_string()),
    }
}

fn write(e: &Expr, out: &mut String) {
    match e {
        Expr::Identifier(i) => ident(i, out),
        // `t.x` is named `x` (measured: `SELECT t.number` gives `number`).
        Expr::CompoundIdentifier(parts) => {
            if let Some(last) = parts.last() {
                ident(last, out);
            }
        }
        Expr::Value(v) => value(&v.value, out),
        Expr::Nested(inner) => write(inner, out),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => match expr.as_ref() {
            Expr::Value(v) if matches!(v.value, Value::Number(..)) => {
                out.push('-');
                value(&v.value, out);
            }
            other => call(out, "negate", &[other]),
        },
        Expr::UnaryOp {
            op: UnaryOperator::Plus,
            expr,
        } => write(expr, out),
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr,
        } => call(out, "not", &[expr]),
        Expr::BinaryOp { left, op, right } => match operator_function(op) {
            Some(f @ ("and" | "or")) => {
                let mut args = Vec::new();
                flatten(e, op, &mut args);
                call(out, f, &args);
            }
            Some(f) => call(out, f, &[left, right]),
            None => out.push_str(&e.to_string()),
        },
        Expr::IsNull(x) => call(out, "isNull", &[x]),
        Expr::IsNotNull(x) => call(out, "isNotNull", &[x]),
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            out.push_str(if *negated { "notIn(" } else { "in(" });
            write(expr, out);
            out.push_str(", ");
            if list.len() == 1 {
                write(&list[0], out);
            } else {
                tuple(list, out);
            }
            out.push(')');
        }
        Expr::InSubquery {
            expr,
            subquery,
            negated,
        } => {
            out.push_str(if *negated { "notIn(" } else { "in(" });
            write(expr, out);
            match subquery_name(subquery) {
                Some(n) => {
                    out.push_str(", ");
                    out.push_str(&n);
                    out.push(')');
                }
                None => {
                    out.push_str(", (");
                    out.push_str(&subquery.to_string());
                    out.push_str("))");
                }
            }
        }
        Expr::Between {
            expr,
            negated: false,
            low,
            high,
        } => {
            out.push_str("and(");
            call(out, "greaterOrEquals", &[expr, low]);
            out.push_str(", ");
            call(out, "lessOrEquals", &[expr, high]);
            out.push(')');
        }
        Expr::Between {
            expr,
            negated: true,
            low,
            high,
        } => {
            out.push_str("or(");
            call(out, "less", &[expr, low]);
            out.push_str(", ");
            call(out, "greater", &[expr, high]);
            out.push(')');
        }
        Expr::Like {
            negated,
            expr,
            pattern,
            ..
        } => call(
            out,
            if *negated { "notLike" } else { "like" },
            &[expr, pattern],
        ),
        Expr::ILike {
            negated,
            expr,
            pattern,
            ..
        } => call(
            out,
            if *negated { "notILike" } else { "ilike" },
            &[expr, pattern],
        ),
        Expr::Cast {
            kind,
            expr,
            data_type,
            ..
        } => {
            let target = crate::context::type_text(data_type);
            out.push_str("CAST(");
            match (kind, expr.as_ref()) {
                // `1::String` casts the literal's text.
                (CastKind::DoubleColon, Expr::Value(v)) if matches!(v.value, Value::Number(..)) => {
                    if let Value::Number(n, _) = &v.value {
                        out.push_str(&quote(n));
                    }
                }
                (_, x) => write(x, out),
            }
            out.push_str(", ");
            out.push_str(&quote(&target));
            out.push(')');
        }
        Expr::Case {
            operand: None,
            conditions,
            else_result,
            ..
        } => {
            out.push_str("multiIf(");
            for (i, c) in conditions.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(&c.condition, out);
                out.push_str(", ");
                write(&c.result, out);
            }
            out.push_str(", ");
            match else_result {
                Some(e) => write(e, out),
                None => out.push_str("NULL"),
            }
            out.push(')');
        }
        Expr::Case {
            operand: Some(op),
            conditions,
            else_result,
            ..
        } => {
            out.push_str("caseWithExpression(");
            write(op, out);
            for c in conditions {
                out.push_str(", ");
                write(&c.condition, out);
                out.push_str(", ");
                write(&c.result, out);
            }
            out.push_str(", ");
            match else_result {
                Some(e) => write(e, out),
                None => out.push_str("NULL"),
            }
            out.push(')');
        }
        Expr::Tuple(items) => tuple(items, out),
        Expr::Array(a) => {
            out.push('[');
            for (i, x) in a.elem.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                // ClickHouse prints the array's literals as its element type:
                // `[1.5, 2]` is `[1.5, 2.]`.
                let float = a.elem.iter().any(is_float_literal);
                match x {
                    Expr::Value(v) if float && matches!(v.value, Value::Number(..)) => {
                        if let Value::Number(n, _) = &v.value {
                            let f: f64 = n.parse().unwrap_or(0.0);
                            out.push_str(&number_text(&format!("{f:e}")));
                        }
                    }
                    other => write(other, out),
                }
            }
            out.push(']');
        }
        Expr::CompoundFieldAccess { root, access_chain } => {
            let mut text = column_name(root);
            for a in access_chain {
                text = match a {
                    AccessExpr::Subscript(Subscript::Index { index }) => {
                        format!("arrayElement({text}, {})", column_name(index))
                    }
                    AccessExpr::Dot(Expr::Identifier(i)) => format!("{text}.{}", i.value),
                    other => format!("{text}{other}"),
                };
            }
            out.push_str(&text);
        }
        Expr::Interval(iv) => {
            let unit = match &iv.leading_field {
                Some(f) => interval_unit(f),
                None => "Second".to_string(),
            };
            out.push_str(&format!("toInterval{unit}("));
            write(&iv.value, out);
            out.push(')');
        }
        Expr::Extract { field, expr, .. } => {
            let f = match field {
                DateTimeField::Year | DateTimeField::Years => "toYear",
                DateTimeField::Quarter => "toQuarter",
                DateTimeField::Month | DateTimeField::Months => "toMonth",
                DateTimeField::Day | DateTimeField::Days => "toDayOfMonth",
                DateTimeField::Hour | DateTimeField::Hours => "toHour",
                DateTimeField::Minute | DateTimeField::Minutes => "toMinute",
                DateTimeField::Second | DateTimeField::Seconds => "toSecond",
                _ => "extract",
            };
            call(out, f, &[expr]);
        }
        Expr::Lambda(LambdaFunction { params, body, .. }) => {
            out.push_str("lambda(tuple(");
            let names: Vec<&Ident> = match params {
                OneOrManyWithParens::One(p) => vec![&p.name],
                OneOrManyWithParens::Many(ps) => ps.iter().map(|p| &p.name).collect(),
            };
            for (i, n) in names.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                ident(n, out);
            }
            out.push_str("), ");
            write(body, out);
            out.push(')');
        }
        Expr::Function(f) => function(f, out),
        Expr::Substring {
            expr,
            substring_from,
            substring_for,
            ..
        } => {
            let mut args: Vec<&Expr> = vec![expr];
            args.extend(substring_from.as_deref());
            args.extend(substring_for.as_deref());
            call(out, "substring", &args);
        }
        // `{'k': v}` is ClickHouse's `map('k', v)` (plan HE1 Task 32).
        Expr::Dictionary(fields) => {
            out.push_str("map(");
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                match f.key.quote_style {
                    Some('\'') => out.push_str(&quote(&f.key.value)),
                    _ => ident(&f.key, out),
                }
                out.push_str(", ");
                write(&f.value, out);
            }
            out.push(')');
        }
        Expr::Subquery(q) => match subquery_name(q) {
            Some(n) => out.push_str(&n),
            None => {
                out.push('(');
                out.push_str(&q.to_string());
                out.push(')');
            }
        },
        other => out.push_str(&other.to_string()),
    }
}

fn is_float_literal(e: &Expr) -> bool {
    matches!(e, Expr::Value(v) if matches!(&v.value, Value::Number(n, _) if n.contains(['.', 'e', 'E'])))
}

fn interval_unit(f: &DateTimeField) -> String {
    let unit = match f {
        DateTimeField::Year | DateTimeField::Years => "Year",
        DateTimeField::Quarter => "Quarter",
        DateTimeField::Month | DateTimeField::Months => "Month",
        DateTimeField::Week(_) | DateTimeField::Weeks => "Week",
        DateTimeField::Day | DateTimeField::Days => "Day",
        DateTimeField::Hour | DateTimeField::Hours => "Hour",
        DateTimeField::Minute | DateTimeField::Minutes => "Minute",
        DateTimeField::Second | DateTimeField::Seconds => "Second",
        DateTimeField::Millisecond | DateTimeField::Milliseconds => "Millisecond",
        DateTimeField::Microsecond | DateTimeField::Microseconds => "Microsecond",
        DateTimeField::Nanosecond | DateTimeField::Nanoseconds => "Nanosecond",
        other => return other.to_string(),
    };
    unit.to_string()
}

fn tuple(items: &[Expr], out: &mut String) {
    out.push('(');
    for (i, x) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write(x, out);
    }
    out.push(')');
}

fn function_args(args: &FunctionArguments) -> (bool, Vec<&Expr>, bool) {
    match args {
        FunctionArguments::None => (false, Vec::new(), false),
        FunctionArguments::Subquery(_) => (false, Vec::new(), false),
        FunctionArguments::List(list) => {
            let distinct = matches!(
                list.duplicate_treatment,
                Some(datafusion::sql::sqlparser::ast::DuplicateTreatment::Distinct)
            );
            let mut star = false;
            let exprs = list
                .args
                .iter()
                .filter_map(|a| match a {
                    FunctionArg::Unnamed(FunctionArgExpr::Expr(e))
                    | FunctionArg::Named {
                        arg: FunctionArgExpr::Expr(e),
                        ..
                    } => Some(e),
                    FunctionArg::Unnamed(FunctionArgExpr::Wildcard) => {
                        star = true;
                        None
                    }
                    _ => None,
                })
                .collect();
            (distinct, exprs, star)
        }
    }
}

fn function(f: &Function, out: &mut String) {
    let name = f.name.to_string();
    let name = name.trim_matches('"').trim_matches('`');
    let (distinct, args, _star) = function_args(&f.args);
    // `(x AS n)` is named `n` (plan HE1 Task 25, [`crate::rewrite`]).
    if name == "__ch_alias"
        && let [_, Expr::Identifier(n)] = args.as_slice()
    {
        out.push_str(&n.value);
        return;
    }
    // The internal names of [`crate::functions::internal_name`].
    let name = name.strip_prefix("__ch_").unwrap_or(name);
    let name = if distinct && name.eq_ignore_ascii_case("count") {
        "countDistinct".to_string()
    } else {
        name.to_string()
    };
    // `tuple(1)` prints as the tuple literal `(1)`.
    if name == "tuple" {
        out.push('(');
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write(a, out);
        }
        out.push(')');
        return;
    }
    out.push_str(&name);
    if let FunctionArguments::List(_) = &f.parameters {
        let (_, params, _) = function_args(&f.parameters);
        out.push('(');
        for (i, p) in params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write(p, out);
        }
        out.push(')');
    }
    out.push('(');
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write(a, out);
    }
    out.push(')');
    if let Some(WindowType::WindowSpec(spec)) = &f.over {
        out.push_str(" OVER (");
        let mut parts = Vec::new();
        if !spec.partition_by.is_empty() {
            parts.push(format!(
                "PARTITION BY {}",
                spec.partition_by
                    .iter()
                    .map(column_name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !spec.order_by.is_empty() {
            parts.push(format!(
                "ORDER BY {}",
                spec.order_by
                    .iter()
                    .map(|o| format!(
                        "{} {}",
                        column_name(&o.expr),
                        if o.options.asc == Some(false) {
                            "DESC"
                        } else {
                            "ASC"
                        }
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.push_str(&parts.join(" "));
        out.push(')');
    } else if let Some(WindowType::NamedWindow(w)) = &f.over {
        out.push_str(" OVER ");
        out.push_str(&w.value);
    }
}
