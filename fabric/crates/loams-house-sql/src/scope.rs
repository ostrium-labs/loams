//! ClickHouse name resolution (plan HE1 Task 25): an AST rewrite run after
//! the guard and before `SqlToRel`, over the catalog's columns.
//!
//! * **Matchers are expanded**: `*`, `t.*` and `COLUMNS('regexp')` /
//!   `COLUMNS(a, b)`, with their transformers `EXCEPT [STRICT]`, `REPLACE`
//!   and `APPLY` in order, into explicit select items named as ClickHouse
//!   names them (a clashing column of a later table is `t.c`). `untuple(t)`
//!   becomes one `tupleElement` per field. What `*` includes is the
//!   catalog's columns; `asterisk_include_alias_columns` and
//!   `asterisk_include_materialized_columns` need the lake tables' column
//!   kinds (Task 26) and settings (Task 37).
//! * **Aliases are visible in every clause** of their query: other select
//!   items, `WHERE`, `GROUP BY`, `HAVING`, `ORDER BY`, `JOIN … ON`. An
//!   alias shadows a column of the same name (`prefer_column_name_to_alias
//!   = 0`); inside its own definition (`f(x) AS x`), and in any alias it
//!   leads to, the name is the column again, as the analyzer resolves it.
//!   Lambda parameters shadow both.
//! * **Scalar `WITH`** (`WITH expr AS n`, `WITH (subquery) AS n`; marker
//!   CTEs of [`crate::rewrite`]) is substituted: at its own level it
//!   shadows columns, in subqueries their own columns come first.
//! * **Duplicate output names** are kept as ClickHouse keeps them: the plan
//!   gets unique internal names and [`resolve`] returns the real ones.

use std::collections::{HashMap, HashSet};

use datafusion::arrow::datatypes::DataType;
use datafusion::sql::sqlparser::ast::{
    AccessExpr, CaseWhen, Distinct, ExceptSelectItem, Expr, Function, FunctionArg, FunctionArgExpr,
    FunctionArgumentList, FunctionArguments, GroupByExpr, Ident, Join, JoinConstraint,
    JoinOperator, NamedWindowDefinition, NamedWindowExpr, ObjectName, ObjectNamePart,
    OneOrManyWithParens, OrderBy, OrderByKind, Query, ReplaceSelectElement, Select, SelectItem,
    SelectItemQualifiedWildcardKind, SetExpr, Statement, Subscript, TableFactor, TableWithJoins,
    Value, WildcardAdditionalOptions, WindowType,
};
use datafusion::sql::sqlparser::tokenizer::Span;
use loams_house_types::ChError;

use crate::context::Catalog;
use crate::names;

/// A visitor over the expressions the guard lets through: [`children`]
/// calls `expr` on each child expression and `query` on each subquery.
pub trait Walk {
    /// Visits one expression.
    fn expr(&mut self, e: &mut Expr) -> Result<(), ChError>;
    /// Visits a subquery inside an expression.
    fn query(&mut self, q: &mut Query) -> Result<(), ChError>;
}

/// Visits `e`'s direct children (subqueries included, through
/// [`Walk::query`]).
pub fn children<W: Walk + ?Sized>(e: &mut Expr, w: &mut W) -> Result<(), ChError> {
    match e {
        Expr::Nested(x)
        | Expr::IsNull(x)
        | Expr::IsNotNull(x)
        | Expr::UnaryOp { expr: x, .. }
        | Expr::Cast { expr: x, .. }
        | Expr::Extract { expr: x, .. } => w.expr(x),
        Expr::BinaryOp { left, right, .. } => {
            w.expr(left)?;
            w.expr(right)
        }
        Expr::InList { expr, list, .. } => {
            w.expr(expr)?;
            list.iter_mut().try_for_each(|x| w.expr(x))
        }
        Expr::InSubquery { expr, subquery, .. } => {
            w.expr(expr)?;
            w.query(subquery)
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            w.expr(expr)?;
            w.expr(low)?;
            w.expr(high)
        }
        Expr::Like { expr, pattern, .. } | Expr::ILike { expr, pattern, .. } => {
            w.expr(expr)?;
            w.expr(pattern)
        }
        Expr::Case {
            operand,
            conditions,
            else_result,
            ..
        } => {
            if let Some(o) = operand {
                w.expr(o)?;
            }
            for CaseWhen { condition, result } in conditions {
                w.expr(condition)?;
                w.expr(result)?;
            }
            if let Some(x) = else_result {
                w.expr(x)?;
            }
            Ok(())
        }
        Expr::Tuple(items) => items.iter_mut().try_for_each(|x| w.expr(x)),
        Expr::Array(a) => a.elem.iter_mut().try_for_each(|x| w.expr(x)),
        Expr::Function(f) => function_children(f, w),
        Expr::Subquery(q) | Expr::Exists { subquery: q, .. } => w.query(q),
        Expr::Lambda(l) => w.expr(&mut l.body),
        Expr::Interval(iv) => w.expr(&mut iv.value),
        Expr::CompoundFieldAccess { root, access_chain } => {
            w.expr(root)?;
            for a in access_chain {
                if let AccessExpr::Subscript(s) = a {
                    match s {
                        Subscript::Index { index } => w.expr(index)?,
                        Subscript::Slice {
                            lower_bound,
                            upper_bound,
                            stride,
                        } => {
                            for x in [lower_bound, upper_bound, stride].into_iter().flatten() {
                                w.expr(x)?;
                            }
                        }
                    }
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn function_children<W: Walk + ?Sized>(f: &mut Function, w: &mut W) -> Result<(), ChError> {
    for args in [&mut f.parameters, &mut f.args] {
        match args {
            FunctionArguments::None => {}
            FunctionArguments::Subquery(q) => w.query(q)?,
            FunctionArguments::List(list) => {
                for a in &mut list.args {
                    if let FunctionArg::Unnamed(FunctionArgExpr::Expr(x))
                    | FunctionArg::Named {
                        arg: FunctionArgExpr::Expr(x),
                        ..
                    }
                    | FunctionArg::ExprNamed {
                        arg: FunctionArgExpr::Expr(x),
                        ..
                    } = a
                    {
                        w.expr(x)?;
                    }
                }
            }
        }
    }
    if let Some(WindowType::WindowSpec(spec)) = &mut f.over {
        for x in &mut spec.partition_by {
            w.expr(x)?;
        }
        for o in &mut spec.order_by {
            w.expr(&mut o.expr)?;
        }
    }
    Ok(())
}

/// The expression of a scalar `WITH` marker CTE
/// (`n AS (SELECT expr AS __ch_with)`, [`crate::rewrite::scalar_with`]).
pub fn scalar_with_expr(q: &mut Query) -> Option<&mut Expr> {
    if q.with.is_some() {
        return None;
    }
    let SetExpr::Select(s) = &mut *q.body else {
        return None;
    };
    if !s.from.is_empty() {
        return None;
    }
    match s.projection.as_mut_slice() {
        [SelectItem::ExprWithAlias { expr, alias }] if alias.value == "__ch_with" => Some(expr),
        _ => None,
    }
}

/// An output column of a table or a query.
#[derive(Debug, Clone)]
struct Col {
    /// ClickHouse's name.
    name: String,
    /// The name in the plan (unique within its query).
    internal: String,
    /// Its type, when a catalog table gives it.
    data_type: Option<DataType>,
}

/// A table expression of a `FROM`.
#[derive(Debug)]
struct Source {
    /// The identifiers a column of it is qualified with.
    qualifier: Vec<Ident>,
    /// What `t.*` and a clashing column's name call it.
    display: String,
    /// The names `t.*` matches (the alias, or the table's name as written).
    names: Vec<String>,
    /// Its columns, when known before planning.
    cols: Option<Vec<Col>>,
    /// `USING` columns of a join's right side, which `*` leaves out.
    hidden: HashSet<String>,
}

type Aliases = HashMap<String, Expr>;

fn ch(code: i32, name: &'static str, message: impl Into<String>) -> ChError {
    ChError {
        code,
        name,
        message: message.into(),
    }
}

/// A generated name (no span): never an alias the query can refer to.
fn generated(name: &str) -> Ident {
    Ident::with_quote('"', name)
}

fn is_generated(i: &Ident) -> bool {
    i.span == Span::empty()
}

fn string_arg(s: &str) -> FunctionArg {
    FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(
        Value::SingleQuotedString(s.to_string()).into(),
    )))
}

fn call(name: Ident, args: Vec<FunctionArg>) -> Expr {
    Expr::Function(Function {
        name: ObjectName::from(vec![name]),
        uses_odbc_syntax: false,
        parameters: FunctionArguments::None,
        args: FunctionArguments::List(FunctionArgumentList {
            duplicate_treatment: None,
            args,
            clauses: vec![],
        }),
        filter: None,
        null_treatment: None,
        over: None,
        within_group: vec![],
    })
}

fn function_name(f: &Function) -> Option<&str> {
    match f.name.0.as_slice() {
        [ObjectNamePart::Identifier(i)] => Some(i.value.as_str()),
        _ => None,
    }
}

fn plain_args(f: &Function) -> Vec<&Expr> {
    match &f.args {
        FunctionArguments::List(list) => list
            .args
            .iter()
            .filter_map(|a| match a {
                FunctionArg::Unnamed(FunctionArgExpr::Expr(x)) => Some(x),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn is_columns(e: &Expr) -> bool {
    matches!(e, Expr::Function(f) if function_name(f).is_some_and(|n| n.eq_ignore_ascii_case("columns")))
}

fn regex(pattern: &str) -> Result<regex::Regex, ChError> {
    regex::Regex::new(pattern).map_err(|e| {
        ch(
            427,
            "CANNOT_COMPILE_REGEXP",
            format!("Cannot compile regexp {pattern}: {e}"),
        )
    })
}

/// Resolves names in `statement` (a query) over `catalog`, `database` being
/// the current database. Returns the output names when the plan's differ
/// (duplicates, which the plan must name apart).
///
/// # Errors
///
/// ClickHouse's error for a name that cannot be resolved as written.
pub fn resolve(
    statement: &mut Statement,
    catalog: &dyn Catalog,
    database: &str,
) -> Result<Option<Vec<String>>, ChError> {
    let Statement::Query(q) = statement else {
        return Ok(None);
    };
    let mut scope = Scope {
        catalog,
        database: database.to_string(),
        ctes: Vec::new(),
    };
    let cols = scope.query(q, &Aliases::new())?;
    Ok(cols.and_then(|cols| {
        cols.iter()
            .any(|c| c.name != c.internal)
            .then(|| cols.into_iter().map(|c| c.name).collect())
    }))
}

struct Scope<'c> {
    catalog: &'c dyn Catalog,
    database: String,
    /// CTEs in scope, innermost last, with their columns.
    ctes: Vec<(String, Option<Vec<Col>>)>,
}

/// One item of an expansion.
struct Expanded {
    /// The column's name, which `EXCEPT` and `REPLACE` match.
    column: String,
    /// The item's name.
    name: String,
    expr: Expr,
    /// The alias of a `REPLACE`d column (an alias the query may use).
    replaced: Option<Ident>,
}

enum Transformer {
    Except { names: Vec<String>, strict: bool },
    ExceptRegex(String),
    Replace(Expr, Ident),
    Apply { written: String, internal: String },
    ApplyLambda(Expr),
}

impl Scope<'_> {
    /// Resolves a query; returns its output columns when known.
    fn query(&mut self, q: &mut Query, inherited: &Aliases) -> Result<Option<Vec<Col>>, ChError> {
        let depth = self.ctes.len();
        let mut level = Aliases::new();
        if let Some(w) = &mut q.with {
            let mut kept = Vec::new();
            for mut cte in std::mem::take(&mut w.cte_tables) {
                let name = cte.alias.name.value.clone();
                if let Some(e) = scalar_with_expr(&mut cte.query) {
                    let mut e = e.clone();
                    let mut visible = inherited.clone();
                    visible.extend(level.clone());
                    let none = Aliases::new();
                    Subst::new(self, &none, &level, inherited, &visible).expr(&mut e)?;
                    level.insert(name, Expr::Nested(Box::new(e)));
                } else {
                    let mut visible = inherited.clone();
                    visible.extend(level.clone());
                    let cols = self.query(&mut cte.query, &visible)?;
                    self.ctes.push((name, cols));
                    kept.push(cte);
                }
            }
            if kept.is_empty() {
                q.with = None;
            } else {
                w.cte_tables = kept;
            }
        }
        let cols = match &mut *q.body {
            SetExpr::Select(s) => self.select(s, &level, inherited, q.order_by.as_mut())?,
            other => {
                let mut visible = inherited.clone();
                visible.extend(level.clone());
                let cols = self.set_expr(other, &visible)?;
                if let Some(ob) = &mut q.order_by
                    && let OrderByKind::Expressions(items) = &mut ob.kind
                {
                    let none = Aliases::new();
                    for o in items {
                        Subst::new(self, &none, &level, inherited, &visible).expr(&mut o.expr)?;
                    }
                }
                cols
            }
        };
        self.ctes.truncate(depth);
        Ok(cols)
    }

    fn set_expr(
        &mut self,
        s: &mut SetExpr,
        inherited: &Aliases,
    ) -> Result<Option<Vec<Col>>, ChError> {
        match s {
            SetExpr::Select(sel) => self.select(sel, &Aliases::new(), inherited, None),
            SetExpr::Query(q) => self.query(q, inherited),
            SetExpr::SetOperation { left, right, .. } => {
                let cols = self.set_expr(left, inherited)?;
                self.set_expr(right, inherited)?;
                Ok(cols)
            }
            _ => Ok(None),
        }
    }

    fn cte(&self, name: &str) -> Option<&Option<Vec<Col>>> {
        self.ctes
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, c)| c)
    }

    fn table_columns(&self, name: &ObjectName) -> Option<Vec<Col>> {
        let parts: Vec<&str> = name
            .0
            .iter()
            .map(|p| match p {
                ObjectNamePart::Identifier(i) => Some(i.value.as_str()),
                ObjectNamePart::Function(_) => None,
            })
            .collect::<Option<_>>()?;
        let (db, table) = match parts.as_slice() {
            [t] => {
                if let Some(cols) = self.cte(t) {
                    return cols.clone();
                }
                (self.database.as_str(), *t)
            }
            [db, t] => (*db, *t),
            _ => return None,
        };
        let source = self.catalog.table(db, table)?;
        Some(
            source
                .schema()
                .fields()
                .iter()
                .map(|f| Col {
                    name: f.name().clone(),
                    internal: f.name().clone(),
                    data_type: Some(f.data_type().clone()),
                })
                .collect(),
        )
    }

    fn source(
        &mut self,
        t: &mut TableFactor,
        inherited: &Aliases,
        out: &mut Vec<Source>,
    ) -> Result<(), ChError> {
        match t {
            TableFactor::Table {
                name, alias, args, ..
            } => {
                let cols = if args.is_some() {
                    table_function_columns(name)
                } else {
                    self.table_columns(name)
                };
                let parts: Vec<Ident> = name
                    .0
                    .iter()
                    .filter_map(|p| match p {
                        ObjectNamePart::Identifier(i) => Some(i.clone()),
                        ObjectNamePart::Function(_) => None,
                    })
                    .collect();
                let (qualifier, display, names) = match alias {
                    Some(a) => (
                        vec![a.name.clone()],
                        a.name.value.clone(),
                        vec![a.name.value.clone()],
                    ),
                    None => {
                        let last = parts.last().map(|i| i.value.clone()).unwrap_or_default();
                        let full = parts
                            .iter()
                            .map(|i| i.value.as_str())
                            .collect::<Vec<_>>()
                            .join(".");
                        (parts.clone(), last.clone(), vec![last, full])
                    }
                };
                out.push(Source {
                    qualifier,
                    display,
                    names,
                    cols,
                    hidden: HashSet::new(),
                });
            }
            TableFactor::Derived {
                subquery, alias, ..
            } => {
                let cols = self.query(subquery, inherited)?;
                let (qualifier, display, names) = match alias {
                    Some(a) => (
                        vec![a.name.clone()],
                        a.name.value.clone(),
                        vec![a.name.value.clone()],
                    ),
                    None => (Vec::new(), String::new(), Vec::new()),
                };
                out.push(Source {
                    qualifier,
                    display,
                    names,
                    cols,
                    hidden: HashSet::new(),
                });
            }
            TableFactor::NestedJoin {
                table_with_joins, ..
            } => self.from(table_with_joins, inherited, out)?,
            _ => out.push(Source {
                qualifier: Vec::new(),
                display: String::new(),
                names: Vec::new(),
                cols: None,
                hidden: HashSet::new(),
            }),
        }
        Ok(())
    }

    fn from(
        &mut self,
        t: &mut TableWithJoins,
        inherited: &Aliases,
        out: &mut Vec<Source>,
    ) -> Result<(), ChError> {
        self.source(&mut t.relation, inherited, out)?;
        for j in &mut t.joins {
            let before = out.len();
            self.source(&mut j.relation, inherited, out)?;
            if let Some(JoinConstraint::Using(cols)) = join_constraint(&mut j.join_operator) {
                let using: HashSet<String> = cols
                    .iter()
                    .filter_map(|c| match c.0.last() {
                        Some(ObjectNamePart::Identifier(i)) => Some(i.value.clone()),
                        _ => None,
                    })
                    .collect();
                for s in &mut out[before..] {
                    s.hidden.extend(using.iter().cloned());
                }
            }
        }
        Ok(())
    }

    /// Resolves one `SELECT`; `order_by` is its query's.
    fn select(
        &mut self,
        s: &mut Select,
        level: &Aliases,
        inherited: &Aliases,
        mut order_by: Option<&mut OrderBy>,
    ) -> Result<Option<Vec<Col>>, ChError> {
        // The FROM tree, with its subqueries.
        let mut from_visible = inherited.clone();
        from_visible.extend(level.clone());
        let mut sources = Vec::new();
        for t in &mut s.from {
            self.from(t, &from_visible, &mut sources)?;
        }
        // Inherited names lose to this query's own columns.
        let known = sources.iter().all(|s| s.cols.is_some());
        let mut inherited = inherited.clone();
        if known {
            for src in &sources {
                for c in src.cols.iter().flatten() {
                    inherited.remove(&c.name);
                    inherited.remove(&c.internal);
                }
            }
        }
        let mut for_subqueries = inherited.clone();
        for_subqueries.extend(level.clone());

        // Matchers and untuple.
        let mut items = Vec::new();
        for item in std::mem::take(&mut s.projection) {
            self.expand_item(item, &sources, &mut items)?;
        }
        if items.is_empty() {
            return Err(ch(
                51,
                "EMPTY_LIST_OF_COLUMNS_QUERIED",
                "Empty list of columns in SELECT query",
            ));
        }
        s.projection = items;

        // Aliases: inside expressions first (taken out), then select items.
        let mut inline = Vec::new();
        {
            let mut c = Inline(&mut inline);
            for item in &mut s.projection {
                if let SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } = item
                {
                    c.expr(e)?;
                }
            }
            for e in locations(s, order_by.as_deref_mut()) {
                c.expr(e)?;
            }
        }
        let mut own = Aliases::new();
        let mut add = |name: &str, e: &Expr| -> Result<(), ChError> {
            match own.get(name) {
                Some(prev) if prev.to_string() != e.to_string() => Err(ch(
                    179,
                    "MULTIPLE_EXPRESSIONS_FOR_ALIAS",
                    format!("Multiple expressions {prev} and {e} for alias {name}"),
                )),
                _ => {
                    own.insert(name.to_string(), e.clone());
                    Ok(())
                }
            }
        };
        for item in &s.projection {
            if let SelectItem::ExprWithAlias { expr, alias } = item
                && !is_generated(alias)
            {
                add(&alias.value, expr)?;
            }
        }
        for (name, e) in &inline {
            add(name, e)?;
        }

        // Substitution.
        for item in &mut s.projection {
            match item {
                SelectItem::ExprWithAlias { expr, alias } => {
                    let mut sub = Subst::new(self, &own, level, &inherited, &for_subqueries);
                    if !is_generated(alias) {
                        sub.in_progress.push(alias.value.clone());
                    }
                    sub.expr(expr)?;
                }
                SelectItem::UnnamedExpr(e) => {
                    let before = e.clone();
                    Subst::new(self, &own, level, &inherited, &for_subqueries).expr(e)?;
                    if *e != before
                        && let Expr::Identifier(i) = &before
                    {
                        *item = SelectItem::ExprWithAlias {
                            expr: e.clone(),
                            alias: generated(&i.value),
                        };
                    }
                }
                SelectItem::Wildcard(_)
                | SelectItem::QualifiedWildcard(..)
                | SelectItem::ExprWithAliases { .. } => {}
            }
        }
        for e in locations(s, order_by) {
            Subst::new(self, &own, level, &inherited, &for_subqueries).expr(e)?;
        }

        // Output names, duplicates named apart in the plan.
        let mut cols = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for item in &mut s.projection {
            let name = match item {
                SelectItem::ExprWithAlias { alias, .. } => alias.value.clone(),
                SelectItem::UnnamedExpr(Expr::Identifier(i)) => i.value.clone(),
                SelectItem::UnnamedExpr(Expr::CompoundIdentifier(parts)) => {
                    parts.last().map(|i| i.value.clone()).unwrap_or_default()
                }
                _ => return Ok(None),
            };
            let n = seen.entry(name.clone()).or_insert(0);
            *n += 1;
            let internal = if *n == 1 {
                name.clone()
            } else {
                let internal = format!("__ch_dup{n}_{name}");
                let expr = match std::mem::replace(
                    item,
                    SelectItem::Wildcard(WildcardAdditionalOptions::default()),
                ) {
                    SelectItem::ExprWithAlias { expr, .. } | SelectItem::UnnamedExpr(expr) => expr,
                    other => {
                        *item = other;
                        continue;
                    }
                };
                *item = SelectItem::ExprWithAlias {
                    expr,
                    alias: generated(&internal),
                };
                internal
            };
            cols.push(Col {
                name,
                internal,
                data_type: None,
            });
        }
        Ok(Some(cols))
    }

    /// Expands a select item's matchers and `untuple` into `out`.
    fn expand_item(
        &mut self,
        item: SelectItem,
        sources: &[Source],
        out: &mut Vec<SelectItem>,
    ) -> Result<(), ChError> {
        match item {
            SelectItem::Wildcard(opts) => match self.expand(sources, None, &opts)? {
                Some(items) => out.extend(items),
                None => out.push(SelectItem::Wildcard(opts)),
            },
            SelectItem::QualifiedWildcard(kind, opts) => {
                let target = match &kind {
                    SelectItemQualifiedWildcardKind::ObjectName(n) => Some(n.clone()),
                    SelectItemQualifiedWildcardKind::Expr(_) => None,
                };
                match target {
                    Some(n) => match self.expand(sources, Some(&n), &opts)? {
                        Some(items) => out.extend(items),
                        None => out.push(SelectItem::QualifiedWildcard(kind, opts)),
                    },
                    None => out.push(SelectItem::QualifiedWildcard(kind, opts)),
                }
            }
            SelectItem::UnnamedExpr(e) if is_columns(&e) => {
                let Expr::Function(f) = &e else {
                    unreachable!()
                };
                match matched(sources, &plain_args(f))? {
                    Some(found) => {
                        out.extend(found.into_iter().map(|x| item_of(x.expr, &x.name, None)))
                    }
                    None => out.push(SelectItem::UnnamedExpr(e)),
                }
            }
            SelectItem::ExprWithAlias { expr, alias } if matches!(&expr, Expr::Function(f) if function_name(f) == Some("untuple")) =>
            {
                out.extend(untuple(&expr, Some(&alias), sources)?);
            }
            SelectItem::UnnamedExpr(mut e) => {
                if contains_columns(&e) {
                    let mut x = ColumnsIn {
                        sources,
                        failed: false,
                    };
                    x.expr(&mut e)?;
                    if x.failed {
                        out.push(SelectItem::UnnamedExpr(e));
                    } else {
                        let name = names::column_name(&e);
                        out.push(SelectItem::ExprWithAlias {
                            expr: e,
                            alias: generated(&name),
                        });
                    }
                } else if matches!(&e, Expr::Function(f) if function_name(f) == Some("untuple")) {
                    out.extend(untuple(&e, None, sources)?);
                } else {
                    out.push(SelectItem::UnnamedExpr(e));
                }
            }
            other => out.push(other),
        }
        Ok(())
    }

    /// Expands `*` or `t.*` (`qualified`) with its transformers; `None`
    /// when its tables' columns are not known before planning.
    fn expand(
        &mut self,
        sources: &[Source],
        qualified: Option<&ObjectName>,
        opts: &WildcardAdditionalOptions,
    ) -> Result<Option<Vec<SelectItem>>, ChError> {
        let mut chain = Vec::new();
        let mut columns: Option<Vec<Expr>> = None;
        if let Some(ExceptSelectItem {
            first_element,
            additional_elements,
        }) = &opts.opt_except
        {
            let mut names = Vec::new();
            for i in std::iter::once(first_element).chain(additional_elements) {
                if i.quote_style == Some('\'') {
                    chain.push(Transformer::ExceptRegex(i.value.clone()));
                } else {
                    names.push(i.value.clone());
                }
            }
            if !names.is_empty() {
                chain.push(Transformer::Except {
                    names,
                    strict: false,
                });
            }
        }
        if let Some(r) = &opts.opt_replace {
            for el in &r.items {
                let ReplaceSelectElement {
                    expr, column_name, ..
                } = el.as_ref();
                let marker = match expr {
                    Expr::Identifier(i) => Some((i.value.as_str(), None)),
                    Expr::Function(f) => function_name(f).map(|n| (n, Some(f))),
                    _ => None,
                };
                let t = match marker {
                    Some(("__ch_except", None)) => Transformer::Except {
                        names: vec![column_name.value.clone()],
                        strict: false,
                    },
                    Some(("__ch_except_strict", None)) => Transformer::Except {
                        names: vec![column_name.value.clone()],
                        strict: true,
                    },
                    Some(("__ch_except", Some(f))) => match plain_args(f).as_slice() {
                        [Expr::Value(v)] => match &v.value {
                            Value::SingleQuotedString(re) => Transformer::ExceptRegex(re.clone()),
                            _ => {
                                return Err(ChError::syntax_error(
                                    "EXCEPT expects names or a regular expression",
                                ));
                            }
                        },
                        _ => {
                            return Err(ChError::syntax_error(
                                "EXCEPT expects names or a regular expression",
                            ));
                        }
                    },
                    Some(("__ch_columns", Some(f))) => {
                        columns = Some(plain_args(f).into_iter().cloned().collect());
                        continue;
                    }
                    Some(("__ch_apply", Some(f))) if column_name.value == "__ch_lambda" => {
                        match plain_args(f).as_slice() {
                            [l @ Expr::Lambda(_)] => Transformer::ApplyLambda((*l).clone()),
                            _ => {
                                return Err(ChError::syntax_error(
                                    "APPLY expects a function or a lambda",
                                ));
                            }
                        }
                    }
                    Some(("__ch_apply", Some(f))) => match plain_args(f).as_slice() {
                        [Expr::Value(v)] => match &v.value {
                            Value::SingleQuotedString(w) => Transformer::Apply {
                                written: w.clone(),
                                internal: column_name.value.clone(),
                            },
                            _ => return Err(ChError::syntax_error("APPLY expects a function")),
                        },
                        _ => return Err(ChError::syntax_error("APPLY expects a function")),
                    },
                    _ => Transformer::Replace(expr.clone(), column_name.clone()),
                };
                chain.push(t);
            }
        }
        // The matched columns.
        let mut items: Vec<Expanded> = match columns {
            Some(args) => {
                let refs: Vec<&Expr> = args.iter().collect();
                match matched(sources, &refs)? {
                    Some(found) => found,
                    None => return Ok(None),
                }
            }
            None => {
                let which = match qualified {
                    None => None,
                    Some(n) => {
                        let text =
                            n.0.iter()
                                .map(|p| match p {
                                    ObjectNamePart::Identifier(i) => i.value.clone(),
                                    ObjectNamePart::Function(f) => f.to_string(),
                                })
                                .collect::<Vec<_>>()
                                .join(".");
                        match sources.iter().position(|s| s.names.contains(&text)) {
                            Some(k) => Some(k),
                            None => return Ok(None),
                        }
                    }
                };
                match source_columns(sources, which, qualified.is_none()) {
                    Some(found) => found,
                    None => return Ok(None),
                }
            }
        };
        for t in chain {
            match t {
                Transformer::Except { names, strict } => {
                    if strict {
                        let missing: Vec<&String> = names
                            .iter()
                            .filter(|n| !items.iter().any(|x| &x.column == *n))
                            .collect();
                        if !missing.is_empty() {
                            let list = missing
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            return Err(ch(
                                36,
                                "BAD_ARGUMENTS",
                                format!(
                                    "Strict EXCEPT column transformer EXCEPT {list} expects following column(s) : {list}"
                                ),
                            ));
                        }
                    }
                    items.retain(|x| !names.contains(&x.column));
                }
                Transformer::ExceptRegex(re) => {
                    let re = regex(&re)?;
                    items.retain(|x| !re.is_match(&x.column));
                }
                Transformer::Replace(e, ident) => {
                    for x in items.iter_mut().filter(|x| x.column == ident.value) {
                        x.expr = e.clone();
                        x.replaced = Some(ident.clone());
                    }
                }
                Transformer::Apply { written, internal } => {
                    for x in &mut items {
                        let arg = std::mem::replace(&mut x.expr, Expr::Value(Value::Null.into()));
                        x.expr = call(
                            Ident::with_quote('"', internal.as_str()),
                            vec![FunctionArg::Unnamed(FunctionArgExpr::Expr(arg))],
                        );
                        x.name = format!("{written}({})", x.name);
                        x.replaced = None;
                    }
                }
                Transformer::ApplyLambda(l) => {
                    let Expr::Lambda(l) = l else { unreachable!() };
                    let param = match &l.params {
                        OneOrManyWithParens::One(p) => p.name.value.clone(),
                        OneOrManyWithParens::Many(ps) if ps.len() == 1 => ps[0].name.value.clone(),
                        OneOrManyWithParens::Many(_) => {
                            return Err(ch(
                                42,
                                "NUMBER_OF_ARGUMENTS_DOESNT_MATCH",
                                "A lambda in APPLY takes one argument",
                            ));
                        }
                    };
                    for x in &mut items {
                        let mut body = (*l.body).clone();
                        Replace {
                            name: &param,
                            with: &Expr::Nested(Box::new(x.expr.clone())),
                            bound: 0,
                        }
                        .expr(&mut body)?;
                        let mut named = (*l.body).clone();
                        Replace {
                            name: &param,
                            with: &Expr::Identifier(Ident::new(x.name.as_str())),
                            bound: 0,
                        }
                        .expr(&mut named)?;
                        x.name = names::column_name(&named);
                        x.expr = body;
                        x.replaced = None;
                    }
                }
            }
        }
        Ok(Some(
            items
                .into_iter()
                .map(|x| item_of(x.expr, &x.name, x.replaced))
                .collect(),
        ))
    }
}

/// A select item for an expanded expression named `name`.
fn item_of(expr: Expr, name: &str, replaced: Option<Ident>) -> SelectItem {
    if let Some(alias) = replaced {
        return SelectItem::ExprWithAlias { expr, alias };
    }
    let plain = match &expr {
        Expr::Identifier(i) => i.value == name,
        Expr::CompoundIdentifier(parts) => parts.last().is_some_and(|i| i.value == name),
        _ => false,
    };
    if plain {
        SelectItem::UnnamedExpr(expr)
    } else {
        SelectItem::ExprWithAlias {
            expr,
            alias: generated(name),
        }
    }
}

/// The columns of `sources` (or of source `which` only), in order, named as
/// ClickHouse names them: a column of a later table whose name an earlier
/// table has is `t.c`. `star`: `*` leaves out a join's `USING` columns on
/// its right side.
fn source_columns(sources: &[Source], which: Option<usize>, star: bool) -> Option<Vec<Expanded>> {
    let mut out = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let single = sources.len() == 1;
    for (k, src) in sources.iter().enumerate() {
        let cols = src.cols.as_ref()?;
        if which.is_none_or(|w| w == k) {
            for c in cols {
                if star && src.hidden.contains(&c.name) {
                    continue;
                }
                let name = if k > 0 && seen.contains(c.name.as_str()) && !src.display.is_empty() {
                    format!("{}.{}", src.display, c.name)
                } else {
                    c.name.clone()
                };
                let expr = if single || src.qualifier.is_empty() {
                    Expr::Identifier(Ident::new(c.internal.as_str()))
                } else {
                    let mut parts = src.qualifier.clone();
                    parts.push(Ident::new(c.internal.as_str()));
                    Expr::CompoundIdentifier(parts)
                };
                out.push(Expanded {
                    column: c.name.clone(),
                    name,
                    expr,
                    replaced: None,
                });
            }
        }
        seen.extend(cols.iter().map(|c| c.name.as_str()));
    }
    Some(out)
}

/// What `COLUMNS(args)` matches: a regular expression over every column,
/// or a list of columns.
fn matched(sources: &[Source], args: &[&Expr]) -> Result<Option<Vec<Expanded>>, ChError> {
    match args {
        [Expr::Value(v)] => {
            let Value::SingleQuotedString(re) = &v.value else {
                return Err(ChError::syntax_error(
                    "COLUMNS expects a regular expression or columns",
                ));
            };
            let re = regex(re)?;
            let Some(mut all) = source_columns(sources, None, true) else {
                return Ok(None);
            };
            all.retain(|x| re.is_match(&x.column));
            Ok(Some(all))
        }
        _ => Ok(Some(
            args.iter()
                .map(|e| {
                    let name = names::column_name(e);
                    Expanded {
                        column: name.clone(),
                        name,
                        expr: (*e).clone(),
                        replaced: None,
                    }
                })
                .collect(),
        )),
    }
}

fn contains_columns(e: &Expr) -> bool {
    struct Find(bool);
    impl Walk for Find {
        fn expr(&mut self, e: &mut Expr) -> Result<(), ChError> {
            if is_columns(e) {
                self.0 = true;
                return Ok(());
            }
            children(e, self)
        }
        fn query(&mut self, _q: &mut Query) -> Result<(), ChError> {
            Ok(())
        }
    }
    let mut f = Find(false);
    let _ = f.expr(&mut e.clone());
    f.0
}

/// `COLUMNS(…)` inside an expression: in a function's arguments it becomes
/// one argument per column (`concat(COLUMNS('^[ab]$'))` is `concat(a, b)`);
/// elsewhere it must match one column.
struct ColumnsIn<'a> {
    sources: &'a [Source],
    failed: bool,
}

impl Walk for ColumnsIn<'_> {
    fn expr(&mut self, e: &mut Expr) -> Result<(), ChError> {
        if is_columns(e) {
            let Expr::Function(cf) = &*e else {
                unreachable!()
            };
            match matched(self.sources, &plain_args(cf))? {
                Some(mut found) if found.len() == 1 => *e = found.remove(0).expr,
                Some(found) => {
                    return Err(ch(
                        42,
                        "NUMBER_OF_ARGUMENTS_DOESNT_MATCH",
                        format!(
                            "COLUMNS matched {} columns where one expression is expected",
                            found.len()
                        ),
                    ));
                }
                None => self.failed = true,
            }
            return Ok(());
        }
        if let Expr::Function(f) = e
            && let FunctionArguments::List(list) = &mut f.args
        {
            let mut args = Vec::new();
            for a in std::mem::take(&mut list.args) {
                match a {
                    FunctionArg::Unnamed(FunctionArgExpr::Expr(x)) if is_columns(&x) => {
                        let Expr::Function(cf) = &x else {
                            unreachable!()
                        };
                        match matched(self.sources, &plain_args(cf))? {
                            Some(found) => args.extend(
                                found
                                    .into_iter()
                                    .map(|m| FunctionArg::Unnamed(FunctionArgExpr::Expr(m.expr))),
                            ),
                            None => {
                                self.failed = true;
                                args.push(FunctionArg::Unnamed(FunctionArgExpr::Expr(x)));
                            }
                        }
                    }
                    other => args.push(other),
                }
            }
            list.args = args;
        }
        children(e, self)
    }

    fn query(&mut self, _q: &mut Query) -> Result<(), ChError> {
        Ok(())
    }
}

/// `untuple` of an expression the plan types: left as one select item for
/// [`crate::untuple`] to expand after `SqlToRel` (plan HE1 Task 32).
fn deferred_untuple(e: &Expr, alias: Option<&Ident>) -> SelectItem {
    let alias = match alias.filter(|a| !is_generated(a)) {
        Some(a) => a.clone(),
        None => generated(&names::column_name(e)),
    };
    SelectItem::ExprWithAlias {
        expr: e.clone(),
        alias,
    }
}

/// `untuple(t)`: one `tupleElement(t, 'field')` per field, named
/// `tupleElement(t, 'field')`, or `alias.field` under an alias.
fn untuple(
    e: &Expr,
    alias: Option<&Ident>,
    sources: &[Source],
) -> Result<Vec<SelectItem>, ChError> {
    let Expr::Function(f) = e else { unreachable!() };
    let args = plain_args(f);
    let [arg] = args.as_slice() else {
        return Err(ch(
            42,
            "NUMBER_OF_ARGUMENTS_DOESNT_MATCH",
            "Function untuple expects 1 argument",
        ));
    };
    let arg = (*arg).clone();
    let fields: Vec<String> = match &arg {
        Expr::Tuple(items) => (1..=items.len()).map(|i| i.to_string()).collect(),
        Expr::Function(t) if function_name(t) == Some("tuple") => {
            (1..=plain_args(t).len()).map(|i| i.to_string()).collect()
        }
        Expr::Identifier(i) => {
            let dt = sources
                .iter()
                .filter_map(|s| s.cols.as_ref())
                .flatten()
                .find(|c| c.name == i.value)
                .and_then(|c| c.data_type.clone());
            match dt {
                Some(DataType::Struct(fields)) => fields.iter().map(|f| f.name().clone()).collect(),
                Some(_) => {
                    return Err(ch(
                        43,
                        "ILLEGAL_TYPE_OF_ARGUMENT",
                        format!(
                            "Argument of function untuple must be a tuple, got the column {}",
                            i.value
                        ),
                    ));
                }
                None => {
                    return Ok(vec![deferred_untuple(e, alias)]);
                }
            }
        }
        _ => {
            return Ok(vec![deferred_untuple(e, alias)]);
        }
    };
    let base = names::column_name(&arg);
    Ok(fields
        .into_iter()
        .map(|field| {
            let name = match alias.filter(|a| !is_generated(a)) {
                Some(a) => format!("{}.{field}", a.value),
                None => format!(
                    "tupleElement({base}, {})",
                    loams_house_types::ty::quote(&field)
                ),
            };
            SelectItem::ExprWithAlias {
                expr: call(
                    Ident::with_quote('"', "tupleElement"),
                    vec![
                        FunctionArg::Unnamed(FunctionArgExpr::Expr(arg.clone())),
                        string_arg(&field),
                    ],
                ),
                alias: generated(&name),
            }
        })
        .collect())
}

/// The clauses of a select other than its items, where aliases are
/// visible: `WHERE`, `GROUP BY`, `HAVING`, `QUALIFY`, `JOIN … ON`,
/// `DISTINCT ON`, named windows, and its query's `ORDER BY`.
fn locations<'a>(s: &'a mut Select, order_by: Option<&'a mut OrderBy>) -> Vec<&'a mut Expr> {
    let Select {
        distinct,
        from,
        prewhere,
        selection,
        group_by,
        having,
        named_window,
        qualify,
        ..
    } = s;
    let mut out: Vec<&mut Expr> = Vec::new();
    if let Some(Distinct::On(exprs)) = distinct {
        out.extend(exprs.iter_mut());
    }
    for t in from.iter_mut() {
        on_exprs(t, &mut out);
    }
    out.extend(prewhere.iter_mut());
    out.extend(selection.iter_mut());
    if let GroupByExpr::Expressions(exprs, _) = group_by {
        out.extend(exprs.iter_mut());
    }
    out.extend(having.iter_mut());
    out.extend(qualify.iter_mut());
    for NamedWindowDefinition(_, w) in named_window.iter_mut() {
        if let NamedWindowExpr::WindowSpec(spec) = w {
            out.extend(spec.partition_by.iter_mut());
            out.extend(spec.order_by.iter_mut().map(|o| &mut o.expr));
        }
    }
    if let Some(ob) = order_by
        && let OrderByKind::Expressions(items) = &mut ob.kind
    {
        out.extend(items.iter_mut().map(|o| &mut o.expr));
    }
    out
}

fn on_exprs<'a>(t: &'a mut TableWithJoins, out: &mut Vec<&'a mut Expr>) {
    let TableWithJoins { relation, joins } = t;
    if let TableFactor::NestedJoin {
        table_with_joins, ..
    } = relation
    {
        on_exprs(table_with_joins, out);
    }
    for Join {
        relation,
        join_operator,
        ..
    } in joins.iter_mut()
    {
        if let TableFactor::NestedJoin {
            table_with_joins, ..
        } = relation
        {
            on_exprs(table_with_joins, out);
        }
        if let Some(JoinConstraint::On(e)) = join_constraint(join_operator) {
            out.push(e);
        }
    }
}

fn join_constraint(op: &mut JoinOperator) -> Option<&mut JoinConstraint> {
    match op {
        JoinOperator::Join(c)
        | JoinOperator::Inner(c)
        | JoinOperator::Left(c)
        | JoinOperator::LeftOuter(c)
        | JoinOperator::Right(c)
        | JoinOperator::RightOuter(c)
        | JoinOperator::FullOuter(c)
        | JoinOperator::CrossJoin(c)
        | JoinOperator::Semi(c)
        | JoinOperator::LeftSemi(c)
        | JoinOperator::RightSemi(c)
        | JoinOperator::Anti(c)
        | JoinOperator::LeftAnti(c)
        | JoinOperator::RightAnti(c)
        | JoinOperator::StraightJoin(c) => Some(c),
        JoinOperator::AsOf { constraint, .. } => Some(constraint),
        _ => None,
    }
}

/// Takes `__ch_alias(x, n)` out of expressions (the alias is `x`),
/// recording `(n, x)`; never inside subqueries.
struct Inline<'a>(&'a mut Vec<(String, Expr)>);

impl Walk for Inline<'_> {
    fn expr(&mut self, e: &mut Expr) -> Result<(), ChError> {
        if let Expr::Function(f) = e
            && function_name(f) == Some("__ch_alias")
        {
            let (mut x, name) = match plain_args(f).as_slice() {
                [x, Expr::Identifier(n)] => ((*x).clone(), n.value.clone()),
                _ => return Err(ChError::syntax_error("Expected an alias")),
            };
            self.expr(&mut x)?;
            self.0.push((name, x.clone()));
            *e = Expr::Nested(Box::new(x));
            return Ok(());
        }
        children(e, self)
    }

    fn query(&mut self, _q: &mut Query) -> Result<(), ChError> {
        Ok(())
    }
}

/// Replaces the identifier `name` with `with`, except where a lambda binds
/// the name again.
struct Replace<'a> {
    name: &'a str,
    with: &'a Expr,
    bound: usize,
}

impl Walk for Replace<'_> {
    fn expr(&mut self, e: &mut Expr) -> Result<(), ChError> {
        match e {
            Expr::Identifier(i) if i.value == self.name && self.bound == 0 => {
                *e = self.with.clone();
                Ok(())
            }
            Expr::Lambda(l) => {
                let binds = lambda_params(&l.params).iter().any(|p| p == self.name);
                self.bound += usize::from(binds);
                let r = self.expr(&mut l.body);
                self.bound -= usize::from(binds);
                r
            }
            _ => children(e, self),
        }
    }

    fn query(&mut self, _q: &mut Query) -> Result<(), ChError> {
        Ok(())
    }
}

fn lambda_params(
    p: &OneOrManyWithParens<datafusion::sql::sqlparser::ast::LambdaFunctionParameter>,
) -> Vec<String> {
    match p {
        OneOrManyWithParens::One(p) => vec![p.name.value.clone()],
        OneOrManyWithParens::Many(ps) => ps.iter().map(|p| p.name.value.clone()).collect(),
    }
}

/// Alias substitution in one clause of a select.
struct Subst<'s, 'c> {
    scope: &'s mut Scope<'c>,
    /// The select's own aliases, as written: resolved where used.
    own: &'s Aliases,
    /// This query level's scalar `WITH`, resolved.
    level: &'s Aliases,
    /// Outer levels' scalar `WITH`, resolved, less this query's columns.
    inherited: &'s Aliases,
    /// What a subquery inherits.
    for_subqueries: &'s Aliases,
    /// Aliases being resolved: inside them, their names are columns.
    in_progress: Vec<String>,
    /// Lambda parameters in scope.
    bound: Vec<String>,
}

impl<'s, 'c> Subst<'s, 'c> {
    fn new(
        scope: &'s mut Scope<'c>,
        own: &'s Aliases,
        level: &'s Aliases,
        inherited: &'s Aliases,
        for_subqueries: &'s Aliases,
    ) -> Self {
        Self {
            scope,
            own,
            level,
            inherited,
            for_subqueries,
            in_progress: Vec::new(),
            bound: Vec::new(),
        }
    }
}

impl Walk for Subst<'_, '_> {
    fn expr(&mut self, e: &mut Expr) -> Result<(), ChError> {
        match e {
            Expr::Identifier(i) if !self.bound.contains(&i.value) => {
                let name = i.value.clone();
                if !self.in_progress.contains(&name)
                    && let Some(raw) = self.own.get(&name)
                {
                    if self.in_progress.len() > 256 {
                        return Err(ch(
                            174,
                            "CYCLIC_ALIASES",
                            format!("Cyclic aliases for {name}"),
                        ));
                    }
                    let mut x = raw.clone();
                    self.in_progress.push(name);
                    self.expr(&mut x)?;
                    self.in_progress.pop();
                    *e = Expr::Nested(Box::new(x));
                    return Ok(());
                }
                if let Some(x) = self.level.get(&name).or_else(|| self.inherited.get(&name)) {
                    *e = x.clone();
                }
                Ok(())
            }
            Expr::Lambda(l) => {
                let params = lambda_params(&l.params);
                let n = self.bound.len();
                self.bound.extend(params);
                let r = self.expr(&mut l.body);
                self.bound.truncate(n);
                r
            }
            _ => children(e, self),
        }
    }

    fn query(&mut self, q: &mut Query) -> Result<(), ChError> {
        self.scope.query(q, self.for_subqueries).map(|_| ())
    }
}

/// The columns of a table function the guard lets through (plan HE1 Task
/// 38): `numbers*` give `number UInt64`, `zeros*` give `zero UInt8`.
fn table_function_columns(name: &ObjectName) -> Option<Vec<Col>> {
    let written = match name.0.as_slice() {
        [ObjectNamePart::Identifier(i)] => i.value.as_str(),
        _ => return None,
    };
    let (column, data_type) = match written {
        "numbers" | "numbers_mt" => ("number", DataType::UInt64),
        "zeros" | "zeros_mt" => ("zero", DataType::UInt8),
        _ => return None,
    };
    Some(vec![Col {
        name: column.to_string(),
        internal: column.to_string(),
        data_type: Some(data_type),
    }])
}
