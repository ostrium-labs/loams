//! The silent-drop guard (plan HE1 Task 22, step 3; dialect spike §3.1,
//! R10.5).
//!
//! DataFusion's `SqlToRel` ignores some fields of sqlparser's AST without an
//! error: `PREWHERE`, `GROUP BY … WITH TOTALS/ROLLUP/CUBE`, a function's
//! parameters, `SAMPLE`, table versions, and more. Planning such a statement
//! would answer as if the construct were not there. This walk visits every
//! node of a query and, for every field, either lets it through (DataFusion
//! plans it), rewrites it into something DataFusion plans the ClickHouse way,
//! or refuses the statement with `48 NOT_IMPLEMENTED` naming the construct.
//!
//! **Exhaustiveness.** Structs are destructured without `..` and enums matched
//! without a wildcard arm, so a sqlparser upgrade that adds a field or a
//! variant does not compile until someone decides what it means here
//! (`no_ignored_field_reaches_sqltorel`).
//!
//! The rewrites made here:
//! * `PREWHERE p` is folded into `WHERE p AND w` (`p` first);
//! * every function name is resolved to its ClickHouse name (case rules,
//!   aliases, combinators, [`crate::functions`]) and quoted, so DataFusion
//!   looks it up as written; an unknown name is `46`;
//! * `CAST(x, 'T')` and `_CAST(x, 'T')` become `CAST(x AS T)` with the
//!   ClickHouse type text, for the House's type planner;
//! * a select item without an alias gets ClickHouse's column name as its
//!   alias ([`crate::names`]);
//! * a table aliased `FINAL` (sqlparser's misreading, R2.3) is marked final;
//! * matchers (`* EXCEPT/REPLACE/APPLY`, `COLUMNS(…)`), `untuple(…)` and
//!   aliases inside expressions are let through for [`crate::scope`], which
//!   expands and resolves them after the guard (plan HE1 Task 25); an
//!   `APPLY`'s function is resolved here;
//! * `[a, b]`, `(a, b)` and `{'k': v}` become `array(…)`, `tuple(…)` and
//!   `map(…)`, and the subscripts `a[i]` and `m['k']` become
//!   `arrayElement(…)` (plan HE1 Task 32), which DataFusion plans through the
//!   House's own functions rather than its array and struct planners;
//! * lambda parameters are quoted, so DataFusion keeps their case as the body
//!   uses it, and `mapFilter` with a one-parameter lambda (the entry as a
//!   tuple) is planned as `__ch_mapFilter_tuple`;
//! * `POSITION(n IN h)`, `SUBSTRING(s FROM a FOR b)`, `TRIM(… FROM s)`,
//!   `LIKE` and `ILIKE` become the functions ClickHouse reads them as
//!   (`position(h, n)`, `substring(s, a, b)`, `trimBoth(s, c)`, `like(…)`,
//!   `notILike(…)`, …), which carry ClickHouse's semantics (plan HE1
//!   Task 31).

use std::collections::BTreeSet;

use datafusion::sql::sqlparser::ast::{
    AccessExpr, Array, BinaryOperator, CaseWhen, CastKind, Cte, DataType, DictionaryField,
    Distinct, Expr, Function, FunctionArg, FunctionArgExpr, FunctionArgumentList,
    FunctionArguments, GroupByExpr, Ident, Join, JoinConstraint, JoinOperator, LambdaFunction,
    LimitClause, Map, MapEntry, NamedWindowDefinition, NamedWindowExpr, ObjectName, ObjectNamePart,
    Offset, OneOrManyWithParens, OrderBy, OrderByExpr, OrderByKind, OrderByOptions, Query, Select,
    SelectFlavor, SelectItem, SetExpr, Setting, Subscript, TableAlias, TableFactor, TableWithJoins,
    TrimWhereField, UnaryOperator, Value, ValueWithSpan, WindowFrame, WindowSpec, WindowType, With,
};
use loams_house_types::ChError;

use crate::functions;
use crate::lex::{Marks, RelationRef};
use crate::names;

/// A statement's own `SETTINGS` (name, value) and `FORMAT`.
pub type StatementOptions = (Vec<(String, String)>, Option<String>);

/// A table a statement reads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableRead {
    /// Its name's parts, unquoted (`[db, t]` or `[t]`).
    pub name: Vec<String>,
    /// Whether it is read with `FINAL`.
    pub final_: bool,
}

/// What the statement reads and calls: the front pins the tables and holds
/// the functions to the security list (plan HE1 Task 39).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolution {
    /// The tables, in the order they appear.
    pub tables: Vec<TableRead>,
    /// The ClickHouse functions called, by canonical name.
    pub functions: BTreeSet<String>,
}

/// The guard's state while it walks one statement.
#[derive(Debug)]
pub struct Guard<'a> {
    marks: &'a Marks,
    /// What the statement reads and calls.
    pub resolution: Resolution,
    /// The statement's subqueries' `_subquery_N` numbers.
    numbers: names::Numbers,
}

fn refuse(what: impl std::fmt::Display) -> ChError {
    ChError::unsupported(what)
}

/// A call of the ClickHouse function `name` on `args`, for the guard to walk
/// (it resolves and quotes the name).
fn call(name: &str, args: Vec<Expr>) -> Expr {
    Expr::Function(Function {
        name: ObjectName::from(vec![Ident::new(name)]),
        uses_odbc_syntax: false,
        parameters: FunctionArguments::None,
        args: FunctionArguments::List(FunctionArgumentList {
            duplicate_treatment: None,
            args: args
                .into_iter()
                .map(|a| FunctionArg::Unnamed(FunctionArgExpr::Expr(a)))
                .collect(),
            clauses: vec![],
        }),
        filter: None,
        null_treatment: None,
        over: None,
        within_group: vec![],
    })
}

/// The name `mapFilter` with a one-parameter lambda is planned under (plan
/// HE1 Task 32): the lambda takes each entry as a `Tuple(K, V)`.
pub const MAP_FILTER_TUPLE: &str = "__ch_mapFilter_tuple";

fn one_parameter_lambda_first(args: &FunctionArguments) -> bool {
    let FunctionArguments::List(list) = args else {
        return false;
    };
    match list.args.first() {
        Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Lambda(l)))) => match &l.params {
            OneOrManyWithParens::One(_) => true,
            OneOrManyWithParens::Many(ps) => ps.len() == 1,
        },
        _ => false,
    }
}

/// A refusal for a construct a later HE1 task brings.
fn later(what: &str, task: u32) -> ChError {
    ChError::not_implemented(format!(
        "{what} is not supported by the Loams House engine yet (plan HE1 Task {task})"
    ))
}

/// The function ClickHouse reads `TRIM(…)` as.
pub fn trim_function(side: Option<&TrimWhereField>) -> &'static str {
    match side {
        Some(TrimWhereField::Leading) => "trimLeft",
        Some(TrimWhereField::Trailing) => "trimRight",
        Some(TrimWhereField::Both) | None => "trimBoth",
    }
}

/// The function call a special form is in ClickHouse (plan HE1 Task 31), if
/// `e` is one.
fn special_form(e: &Expr) -> Result<Option<Expr>, ChError> {
    fn b(x: &Expr) -> Expr {
        x.clone()
    }
    Ok(Some(match e {
        Expr::Like {
            negated,
            any,
            expr,
            pattern,
            escape_char,
        }
        | Expr::ILike {
            negated,
            any,
            expr,
            pattern,
            escape_char,
        } => {
            if *any || escape_char.is_some() {
                return Err(refuse("LIKE ANY / ESCAPE"));
            }
            let name = match (matches!(e, Expr::ILike { .. }), *negated) {
                (false, false) => "like",
                (false, true) => "notLike",
                (true, false) => "ilike",
                (true, true) => "notILike",
            };
            call(name, vec![b(expr), b(pattern)])
        }
        Expr::Position { expr, r#in } => call("position", vec![b(r#in), b(expr)]),
        Expr::Substring {
            expr,
            substring_from,
            substring_for,
            special: _,
            shorthand: _,
        } => {
            let Some(from) = substring_from else {
                return Err(ChError::syntax_error("SUBSTRING(… FOR …) needs FROM"));
            };
            let mut args = vec![b(expr), b(from)];
            args.extend(substring_for.as_deref().map(b));
            call("substring", args)
        }
        Expr::Trim {
            trim_where,
            trim_what,
            expr,
            trim_characters,
        } => {
            if trim_where.is_none() && trim_what.is_some() {
                return Err(ChError::syntax_error(
                    "TRIM(c FROM s) needs BOTH, LEADING or TRAILING",
                ));
            }
            let mut args = vec![b(expr)];
            args.extend(trim_what.as_deref().map(b));
            args.extend(trim_characters.iter().flatten().cloned());
            if args.len() > 2 {
                return Err(ChError::number_of_arguments_doesnt_match(
                    "TRIM takes one set of characters",
                ));
            }
            call(trim_function(trim_where.as_ref()), args)
        }
        _ => return Ok(None),
    }))
}

impl<'a> Guard<'a> {
    /// A guard for a statement whose pre-pass gave `marks`.
    pub fn new(marks: &'a Marks) -> Self {
        Self {
            marks,
            resolution: Resolution::default(),
            numbers: names::Numbers::new(),
        }
    }

    /// Walks a top-level query: its `SETTINGS` and `FORMAT` are the
    /// statement's, taken out and returned for the front.
    pub fn top_query(&mut self, q: &mut Query) -> Result<StatementOptions, ChError> {
        let mut settings: Vec<(String, String)> = q
            .settings
            .take()
            .unwrap_or_default()
            .into_iter()
            .map(|Setting { key, value }| (key.value, setting_text(&value)))
            .collect();
        settings.extend(self.marks.settings.iter().cloned());
        let format = q
            .format_clause
            .take()
            .map(|f| match f {
                datafusion::sql::sqlparser::ast::FormatClause::Identifier(i) => i.value,
                datafusion::sql::sqlparser::ast::FormatClause::Null => "Null".to_string(),
            })
            .or_else(|| self.marks.format.clone());
        self.numbers = names::subquery_numbers(q);
        self.query(q)?;
        Ok((settings, format))
    }

    /// Walks a query (a subquery, a CTE's body).
    pub fn query(&mut self, q: &mut Query) -> Result<(), ChError> {
        let Query {
            with,
            body,
            order_by,
            limit_clause,
            fetch,
            locks,
            for_clause,
            settings,
            format_clause,
            pipe_operators,
        } = q;
        if let Some(w) = with {
            self.with(w)?;
        }
        self.set_expr(body)?;
        if let Some(ob) = order_by {
            self.order_by(ob)?;
        }
        if let Some(l) = limit_clause {
            match l {
                LimitClause::LimitOffset {
                    limit,
                    offset,
                    limit_by,
                } => {
                    if !limit_by.is_empty() {
                        return Err(later("LIMIT … BY", 34));
                    }
                    if let Some(e) = limit {
                        self.expr(e)?;
                    }
                    if let Some(Offset { value, rows: _ }) = offset {
                        self.expr(value)?;
                    }
                }
                LimitClause::OffsetCommaLimit { offset, limit } => {
                    self.expr(offset)?;
                    self.expr(limit)?;
                }
            }
        }
        if fetch.is_some() {
            return Err(refuse("FETCH"));
        }
        if !locks.is_empty() {
            return Err(refuse("FOR UPDATE / FOR SHARE"));
        }
        if for_clause.is_some() {
            return Err(refuse("FOR XML / FOR JSON"));
        }
        if settings.is_some() {
            return Err(refuse("SETTINGS inside a subquery"));
        }
        if format_clause.is_some() {
            return Err(refuse("FORMAT inside a subquery"));
        }
        if !pipe_operators.is_empty() {
            return Err(refuse("Pipe operators"));
        }
        Ok(())
    }

    fn with(&mut self, w: &mut With) -> Result<(), ChError> {
        let With {
            with_token: _,
            recursive,
            cte_tables,
        } = w;
        if *recursive {
            return Err(refuse("WITH RECURSIVE"));
        }
        for cte in cte_tables {
            let Cte {
                alias,
                query,
                from,
                materialized,
                closing_paren_token: _,
            } = cte;
            self.alias(alias)?;
            if from.is_some() || materialized.is_some() {
                return Err(refuse("CTE options"));
            }
            self.query(query)?;
        }
        Ok(())
    }

    fn order_by(&mut self, ob: &mut OrderBy) -> Result<(), ChError> {
        let OrderBy { kind, interpolate } = ob;
        if interpolate.is_some() {
            return Err(later("INTERPOLATE", 34));
        }
        match kind {
            OrderByKind::All(_) => Err(refuse("ORDER BY ALL")),
            OrderByKind::Expressions(items) => {
                for item in items {
                    self.order_by_expr(item)?;
                }
                Ok(())
            }
        }
    }

    fn order_by_expr(&mut self, o: &mut OrderByExpr) -> Result<(), ChError> {
        let OrderByExpr {
            expr,
            options:
                OrderByOptions {
                    asc: _,
                    nulls_first: _,
                },
            with_fill,
        } = o;
        if with_fill.is_some() {
            return Err(later("WITH FILL", 34));
        }
        self.expr(expr)
    }

    fn set_expr(&mut self, s: &mut SetExpr) -> Result<(), ChError> {
        match s {
            SetExpr::Select(select) => self.select(select),
            SetExpr::Query(q) => self.query(q),
            SetExpr::SetOperation {
                left,
                op: _,
                set_quantifier: _,
                right,
            } => {
                self.set_expr(left)?;
                self.set_expr(right)
            }
            SetExpr::Values(_) => Err(refuse("VALUES as a query")),
            SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Merge(_) => {
                Err(refuse("A data-changing statement inside a query"))
            }
            SetExpr::Table(_) => Err(refuse("TABLE t as a query")),
        }
    }

    fn select(&mut self, s: &mut Select) -> Result<(), ChError> {
        let Select {
            select_token: _,
            optimizer_hints,
            distinct,
            select_modifiers,
            top,
            top_before_distinct: _,
            projection,
            exclude,
            into,
            from,
            lateral_views,
            prewhere,
            selection,
            connect_by,
            group_by,
            cluster_by,
            distribute_by,
            sort_by,
            having,
            named_window,
            qualify,
            window_before_qualify: _,
            value_table_mode,
            flavor,
        } = s;
        if !optimizer_hints.is_empty() {
            return Err(refuse("Optimizer hints"));
        }
        if select_modifiers.is_some() {
            return Err(refuse("MySQL select modifiers"));
        }
        if top.is_some() {
            return Err(refuse("TOP"));
        }
        if exclude.is_some() {
            return Err(refuse("SELECT … EXCLUDE"));
        }
        if into.is_some() {
            return Err(refuse("SELECT … INTO"));
        }
        if !lateral_views.is_empty() {
            return Err(refuse("LATERAL VIEW"));
        }
        if !connect_by.is_empty() {
            return Err(refuse("CONNECT BY"));
        }
        if !cluster_by.is_empty() || !distribute_by.is_empty() || !sort_by.is_empty() {
            return Err(refuse("CLUSTER BY / DISTRIBUTE BY / SORT BY"));
        }
        if value_table_mode.is_some() {
            return Err(refuse("SELECT AS VALUE / STRUCT"));
        }
        if !matches!(flavor, SelectFlavor::Standard) {
            return Err(refuse("FROM-first SELECT"));
        }
        match distinct {
            None | Some(Distinct::Distinct | Distinct::All) => {}
            Some(Distinct::On(exprs)) => {
                for e in exprs {
                    self.expr(e)?;
                }
            }
        }
        // Names first, from the text as written, before any rewrite. An item
        // with a `COLUMNS` matcher is named once [`crate::scope`] expands it.
        for item in projection.iter_mut() {
            if let SelectItem::UnnamedExpr(e) = item
                && !matches!(e, Expr::Identifier(_))
                && !has_columns_matcher(e)
            {
                let name = names::column_name_numbered(e, &self.numbers);
                let expr = std::mem::replace(e, Expr::Value(Value::Null.into()));
                *item = SelectItem::ExprWithAlias {
                    expr,
                    alias: Ident::with_quote('"', name),
                };
            }
        }
        for item in projection.iter_mut() {
            match item {
                SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, alias: _ } => {
                    self.expr(e)?
                }
                SelectItem::ExprWithAliases { .. } => {
                    return Err(refuse("A select item with several aliases"));
                }
                SelectItem::QualifiedWildcard(_, opts) | SelectItem::Wildcard(opts) => {
                    let datafusion::sql::sqlparser::ast::WildcardAdditionalOptions {
                        wildcard_token: _,
                        opt_ilike,
                        opt_exclude,
                        opt_except,
                        opt_replace,
                        opt_rename,
                        opt_alias,
                    } = opts;
                    if opt_ilike.is_some()
                        || opt_exclude.is_some()
                        || opt_rename.is_some()
                        || opt_alias.is_some()
                    {
                        return Err(refuse("* ILIKE / EXCLUDE / RENAME / AS"));
                    }
                    // `EXCEPT` is names (or a quoted regular expression),
                    // expanded by [`crate::scope`].
                    let _ = opt_except;
                    if let Some(r) = opt_replace {
                        for el in &mut r.items {
                            self.replace_element(el)?;
                        }
                    }
                }
            }
        }
        for twj in from.iter_mut() {
            self.table_with_joins(twj)?;
        }
        // PREWHERE folds into WHERE, its conjuncts first (spike q15; plan HE1
        // Task 35 keeps them ahead of FINAL's deduplication).
        if let Some(p) = prewhere.take() {
            *selection = Some(match selection.take() {
                Some(w) => Expr::BinaryOp {
                    left: Box::new(Expr::Nested(Box::new(p))),
                    op: BinaryOperator::And,
                    right: Box::new(Expr::Nested(Box::new(w))),
                },
                None => p,
            });
        }
        if let Some(w) = selection {
            self.expr(w)?;
        }
        match group_by {
            GroupByExpr::All(mods) => {
                if !mods.is_empty() {
                    return Err(later("GROUP BY … WITH ROLLUP / CUBE / TOTALS", 34));
                }
                return Err(refuse("GROUP BY ALL"));
            }
            GroupByExpr::Expressions(exprs, mods) => {
                if !mods.is_empty() {
                    return Err(later("GROUP BY … WITH ROLLUP / CUBE / TOTALS", 34));
                }
                for e in exprs {
                    self.expr(e)?;
                }
            }
        }
        if let Some(h) = having {
            self.expr(h)?;
        }
        for NamedWindowDefinition(_, w) in named_window {
            match w {
                NamedWindowExpr::NamedWindow(_) => {}
                NamedWindowExpr::WindowSpec(spec) => self.window_spec(spec)?,
            }
        }
        if let Some(q) = qualify {
            self.expr(q)?;
        }
        Ok(())
    }

    /// One element of a matcher's `REPLACE (…)` list: a replacement, or a
    /// marker of [`crate::rewrite::matchers`]. An `APPLY`'s function is
    /// resolved here: the marker becomes `__ch_apply('written') AS "internal"`.
    fn replace_element(
        &mut self,
        el: &mut datafusion::sql::sqlparser::ast::ReplaceSelectElement,
    ) -> Result<(), ChError> {
        let marker = |e: &Expr| match e {
            Expr::Identifier(i) => Some(i.value.clone()),
            Expr::Function(f) => Some(f.name.to_string()),
            _ => None,
        };
        match marker(&el.expr).as_deref() {
            Some("__ch_except" | "__ch_except_strict" | "__ch_columns") => Ok(()),
            Some("__ch_apply") => {
                if el.column_name.value == "__ch_lambda" {
                    let Expr::Function(f) = &mut el.expr else {
                        return Err(refuse("APPLY"));
                    };
                    let FunctionArguments::List(list) = &mut f.args else {
                        return Err(refuse("APPLY"));
                    };
                    for a in &mut list.args {
                        if let FunctionArg::Unnamed(FunctionArgExpr::Expr(x)) = a {
                            self.expr(x)?;
                        }
                    }
                    return Ok(());
                }
                let written = el.column_name.value.clone();
                if matches!(&el.expr, Expr::Function(_)) {
                    return Err(later(
                        &format!("The parametric function {written}(…)(…)"),
                        28,
                    ));
                }
                let known = functions::resolve(&written).ok_or_else(|| {
                    ChError::unknown_function(format!(
                        "Function with name `{written}` does not exist"
                    ))
                })?;
                self.resolution.functions.insert(known.canonical.clone());
                el.expr = Expr::Function(Function {
                    name: ObjectName::from(vec![Ident::new("__ch_apply")]),
                    uses_odbc_syntax: false,
                    parameters: FunctionArguments::None,
                    args: FunctionArguments::List(FunctionArgumentList {
                        duplicate_treatment: None,
                        args: vec![FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(
                            Value::SingleQuotedString(written).into(),
                        )))],
                        clauses: vec![],
                    }),
                    filter: None,
                    null_treatment: None,
                    over: None,
                    within_group: vec![],
                });
                el.column_name = Ident::with_quote('"', functions::internal_name(&known.canonical));
                Ok(())
            }
            _ => self.expr(&mut el.expr),
        }
    }

    fn alias(&mut self, a: &mut TableAlias) -> Result<(), ChError> {
        let TableAlias {
            explicit: _,
            name: _,
            columns,
            at,
        } = a;
        if at.is_some() {
            return Err(refuse("A table alias AT"));
        }
        for c in columns {
            if c.data_type.is_some() {
                return Err(refuse("A typed column alias"));
            }
        }
        Ok(())
    }

    fn table_with_joins(&mut self, t: &mut TableWithJoins) -> Result<(), ChError> {
        let TableWithJoins { relation, joins } = t;
        self.table_factor(relation)?;
        for join in joins {
            let Join {
                relation,
                global: _,
                join_operator,
            } = join;
            self.table_factor(relation)?;
            let constraint = match join_operator {
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
                | JoinOperator::RightAnti(c) => c,
                JoinOperator::ArrayJoin
                | JoinOperator::LeftArrayJoin
                | JoinOperator::InnerArrayJoin => {
                    return Err(later("ARRAY JOIN", 35));
                }
                JoinOperator::AsOf { .. } => return Err(later("ASOF JOIN", 35)),
                JoinOperator::CrossApply | JoinOperator::OuterApply => {
                    return Err(refuse("APPLY joins"));
                }
                JoinOperator::StraightJoin(_) => return Err(refuse("STRAIGHT_JOIN")),
            };
            match constraint {
                JoinConstraint::On(e) => self.expr(e)?,
                JoinConstraint::Using(_) | JoinConstraint::None => {}
                JoinConstraint::Natural => return Err(refuse("NATURAL JOIN")),
            }
        }
        Ok(())
    }

    fn table_factor(&mut self, t: &mut TableFactor) -> Result<(), ChError> {
        match t {
            TableFactor::Table {
                name,
                alias,
                args,
                with_hints,
                version,
                with_ordinality,
                partitions,
                json_path,
                sample,
                index_hints,
            } => {
                if args.is_some() {
                    return Err(later("Table functions", 38));
                }
                if !with_hints.is_empty()
                    || version.is_some()
                    || *with_ordinality
                    || !partitions.is_empty()
                    || json_path.is_some()
                    || !index_hints.is_empty()
                {
                    return Err(refuse(format!("Table options on {name}")));
                }
                if sample.is_some() {
                    return Err(later("SAMPLE", 35));
                }
                // sqlparser reads `FROM t FINAL` as the alias FINAL (R2.3).
                let mut final_ = false;
                if let Some(a) = alias
                    && a.name.quote_style.is_none()
                    && a.name.value.eq_ignore_ascii_case("final")
                {
                    final_ = true;
                    *alias = None;
                }
                if let Some(a) = alias {
                    self.alias(a)?;
                }
                let written = RelationRef {
                    name: name.to_string(),
                    alias: alias.as_ref().map(|a| a.name.value.clone()),
                };
                final_ |= self.marks.finals.contains(&written);
                let parts = name
                    .0
                    .iter()
                    .map(|p| match p {
                        ObjectNamePart::Identifier(i) => Ok(i.value.clone()),
                        ObjectNamePart::Function(_) => Err(refuse("A function in a table name")),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if final_ {
                    return Err(later("FINAL", 27));
                }
                self.resolution.tables.push(TableRead {
                    name: parts,
                    final_,
                });
                Ok(())
            }
            TableFactor::Derived {
                lateral,
                subquery,
                alias,
                sample,
            } => {
                if *lateral {
                    return Err(refuse("LATERAL subqueries"));
                }
                if sample.is_some() {
                    return Err(later("SAMPLE", 35));
                }
                if let Some(a) = alias {
                    self.alias(a)?;
                }
                self.query(subquery)
            }
            TableFactor::NestedJoin {
                table_with_joins,
                alias,
            } => {
                if let Some(a) = alias {
                    self.alias(a)?;
                }
                self.table_with_joins(table_with_joins)
            }
            TableFactor::TableFunction { .. } | TableFactor::Function { .. } => {
                Err(later("Table functions", 38))
            }
            TableFactor::UNNEST { .. } => Err(refuse("UNNEST in FROM")),
            TableFactor::JsonTable { .. } | TableFactor::OpenJsonTable { .. } => {
                Err(refuse("JSON_TABLE"))
            }
            TableFactor::Pivot { .. } | TableFactor::Unpivot { .. } => {
                Err(refuse("PIVOT / UNPIVOT"))
            }
            TableFactor::MatchRecognize { .. } => Err(refuse("MATCH_RECOGNIZE")),
            TableFactor::XmlTable { .. } => Err(refuse("XMLTABLE")),
            TableFactor::SemanticView { .. } => Err(refuse("SEMANTIC_VIEW")),
        }
    }

    fn window_spec(&mut self, w: &mut WindowSpec) -> Result<(), ChError> {
        let WindowSpec {
            window_name: _,
            partition_by,
            order_by,
            window_frame,
        } = w;
        for e in partition_by {
            self.expr(e)?;
        }
        for o in order_by {
            self.order_by_expr(o)?;
        }
        if let Some(WindowFrame {
            units: _,
            start_bound: _,
            end_bound: _,
        }) = window_frame
        {}
        Ok(())
    }

    fn function(&mut self, f: &mut Function) -> Result<Option<Expr>, ChError> {
        let Function {
            name,
            uses_odbc_syntax,
            parameters,
            args,
            filter,
            null_treatment,
            over,
            within_group,
        } = f;
        if *uses_odbc_syntax {
            return Err(refuse("ODBC function syntax"));
        }
        if filter.is_some() {
            return Err(refuse("FILTER (WHERE …) on an aggregate"));
        }
        if null_treatment.is_some() {
            return Err(refuse("IGNORE NULLS / RESPECT NULLS"));
        }
        if !within_group.is_empty() {
            return Err(refuse("WITHIN GROUP"));
        }
        let written = match name.0.as_slice() {
            [ObjectNamePart::Identifier(i)] => i.value.clone(),
            _ => return Err(refuse(format!("The qualified function name {name}"))),
        };
        // Resolved by [`crate::scope`] after the guard: a matcher, `untuple`,
        // and an alias inside an expression (`__ch_alias(x, n)`).
        if matches!(parameters, FunctionArguments::None)
            && (written.eq_ignore_ascii_case("columns")
                || written == "untuple"
                || written == "__ch_alias")
        {
            if written != "untuple" && written != "__ch_alias" {
                return Ok(None);
            }
            if let FunctionArguments::List(list) = args {
                let n = if written == "__ch_alias" {
                    1
                } else {
                    list.args.len()
                };
                for a in list.args.iter_mut().take(n) {
                    if let FunctionArg::Unnamed(FunctionArgExpr::Expr(x)) = a {
                        self.expr(x)?;
                    }
                }
            }
            if written == "untuple" {
                self.resolution.functions.insert("tupleElement".into());
            }
            return Ok(None);
        }
        if !matches!(parameters, FunctionArguments::None) {
            return Err(later(
                &format!("The parametric function {written}(…)(…)"),
                28,
            ));
        }
        // `CAST(x, 'T')`, `_CAST(x, 'T')`: a cast to the ClickHouse type text.
        if (written.eq_ignore_ascii_case("cast") || written == "_CAST")
            && let FunctionArguments::List(list) = args
            && let [
                FunctionArg::Unnamed(FunctionArgExpr::Expr(x)),
                FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(ValueWithSpan {
                    value: Value::SingleQuotedString(t),
                    ..
                }))),
            ] = list.args.as_slice()
            && list.duplicate_treatment.is_none()
            && list.clauses.is_empty()
        {
            let mut x = x.clone();
            self.expr(&mut x)?;
            return Ok(Some(Expr::Cast {
                kind: CastKind::Cast,
                expr: Box::new(x),
                data_type: DataType::Custom(ObjectName::from(vec![Ident::new(t.clone())]), vec![]),
                array: false,
                format: None,
            }));
        }
        let known = functions::resolve(&written).ok_or_else(|| {
            ChError::unknown_function(format!("Function with name `{written}` does not exist"))
        })?;
        self.resolution.functions.insert(known.canonical.clone());
        // ClickHouse's `count()` and `count(*)` count rows: `count(1)` here
        // (the column name was taken from the text before this rewrite).
        if known.canonical == "count" {
            let one = || {
                FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(
                    Value::Number("1".into(), false).into(),
                )))
            };
            match args {
                FunctionArguments::List(list)
                    if list.args.is_empty()
                        || matches!(
                            list.args.as_slice(),
                            [FunctionArg::Unnamed(FunctionArgExpr::Wildcard)]
                        ) =>
                {
                    list.args = vec![one()];
                }
                FunctionArguments::None => {
                    *args = FunctionArguments::List(FunctionArgumentList {
                        duplicate_treatment: None,
                        args: vec![one()],
                        clauses: vec![],
                    });
                }
                _ => {}
            }
        }
        let internal = if known.canonical == "mapFilter" && one_parameter_lambda_first(args) {
            MAP_FILTER_TUPLE.to_string()
        } else {
            functions::internal_name(&known.canonical)
        };
        *name = ObjectName::from(vec![Ident::with_quote('"', internal)]);
        match args {
            FunctionArguments::None => {}
            FunctionArguments::Subquery(q) => self.query(q)?,
            FunctionArguments::List(FunctionArgumentList {
                duplicate_treatment: _,
                args,
                clauses,
            }) => {
                if !clauses.is_empty() {
                    return Err(refuse(format!("Clauses inside the arguments of {written}")));
                }
                for a in args {
                    match a {
                        FunctionArg::Unnamed(e)
                        | FunctionArg::Named { arg: e, .. }
                        | FunctionArg::ExprNamed { arg: e, .. } => match e {
                            FunctionArgExpr::Expr(x) => self.expr(x)?,
                            FunctionArgExpr::Wildcard => {}
                            FunctionArgExpr::QualifiedWildcard(_)
                            | FunctionArgExpr::WildcardWithOptions(_) => {
                                return Err(refuse(format!("A wildcard argument of {written}")));
                            }
                        },
                    }
                }
            }
        }
        match over {
            None | Some(WindowType::NamedWindow(_)) => {}
            Some(WindowType::WindowSpec(spec)) => self.window_spec(spec)?,
        }
        Ok(None)
    }

    /// Walks (and rewrites) an expression.
    pub fn expr(&mut self, e: &mut Expr) -> Result<(), ChError> {
        if let Some(call) = special_form(e)? {
            *e = call;
            return self.expr(e);
        }
        match e {
            Expr::Identifier(_) | Expr::CompoundIdentifier(_) => Ok(()),
            Expr::Value(v) => match &v.value {
                Value::Number(..)
                | Value::SingleQuotedString(_)
                | Value::Boolean(_)
                | Value::Null => Ok(()),
                other => Err(refuse(format!("The literal {other}"))),
            },
            Expr::Nested(x) | Expr::IsNull(x) | Expr::IsNotNull(x) => self.expr(x),
            Expr::UnaryOp { op, expr } => match op {
                UnaryOperator::Minus | UnaryOperator::Plus | UnaryOperator::Not => self.expr(expr),
                other => Err(refuse(format!("The operator {other}"))),
            },
            Expr::BinaryOp { left, op, right } => {
                if names::operator_function(op).is_none() {
                    return Err(refuse(format!("The operator {op}")));
                }
                self.expr(left)?;
                self.expr(right)
            }
            Expr::InList {
                expr,
                list,
                negated: _,
            } => {
                self.expr(expr)?;
                list.iter_mut().try_for_each(|x| self.expr(x))
            }
            Expr::InSubquery {
                expr,
                subquery,
                negated: _,
            } => {
                self.expr(expr)?;
                self.query(subquery)
            }
            Expr::Between {
                expr,
                negated: _,
                low,
                high,
            } => {
                self.expr(expr)?;
                self.expr(low)?;
                self.expr(high)
            }
            // Rewritten to function calls by `special_form` above.
            Expr::Like { .. }
            | Expr::ILike { .. }
            | Expr::Position { .. }
            | Expr::Substring { .. }
            | Expr::Trim { .. } => Err(ChError::logical_error(format!(
                "{e} reached the guard unrewritten"
            ))),
            Expr::Cast {
                kind,
                expr,
                data_type: _,
                array,
                format,
            } => {
                if matches!(kind, CastKind::TryCast | CastKind::SafeCast)
                    || *array
                    || format.is_some()
                {
                    return Err(refuse("TRY_CAST / SAFE_CAST / CAST … FORMAT"));
                }
                self.expr(expr)
            }
            Expr::Case {
                case_token: _,
                end_token: _,
                operand,
                conditions,
                else_result,
            } => {
                if operand.is_some() {
                    return Err(later("CASE x WHEN …", 33));
                }
                for CaseWhen { condition, result } in conditions {
                    self.expr(condition)?;
                    self.expr(result)?;
                }
                if let Some(x) = else_result {
                    self.expr(x)?;
                }
                Ok(())
            }
            Expr::Tuple(items) => {
                *e = call("tuple", std::mem::take(items));
                self.expr(e)
            }
            Expr::Array(Array { elem, named: _ }) => {
                *e = call("array", std::mem::take(elem));
                self.expr(e)
            }
            Expr::Function(f) => {
                if let Some(replacement) = self.function(f)? {
                    *e = replacement;
                }
                Ok(())
            }
            Expr::Subquery(q) => self.query(q),
            Expr::Exists {
                subquery,
                negated: _,
            } => self.query(subquery),
            Expr::Lambda(LambdaFunction {
                params,
                body,
                syntax: _,
            }) => {
                let params: Vec<&mut _> = match params {
                    OneOrManyWithParens::One(p) => vec![p],
                    OneOrManyWithParens::Many(ps) => ps.iter_mut().collect(),
                };
                for p in params {
                    if p.data_type.is_some() {
                        return Err(refuse("Typed lambda parameters"));
                    }
                    p.name.quote_style = Some('"');
                }
                self.expr(body)
            }
            Expr::Interval(_) => Err(later("INTERVAL", 30)),
            Expr::Extract { .. } => Err(later("EXTRACT", 30)),
            Expr::CompoundFieldAccess { root, access_chain } => {
                let mut out = std::mem::replace(root.as_mut(), Expr::Value(Value::Null.into()));
                for access in std::mem::take(access_chain) {
                    out = match access {
                        AccessExpr::Subscript(Subscript::Index { index }) => {
                            call("arrayElement", vec![out, index])
                        }
                        AccessExpr::Dot(Expr::Identifier(i)) => call(
                            "tupleElement",
                            vec![out, Expr::Value(Value::SingleQuotedString(i.value).into())],
                        ),
                        AccessExpr::Dot(Expr::Value(ValueWithSpan {
                            value: Value::Number(n, l),
                            ..
                        })) => call(
                            "tupleElement",
                            vec![out, Expr::Value(Value::Number(n, l).into())],
                        ),
                        AccessExpr::Subscript(Subscript::Slice { .. }) => {
                            return Err(refuse("Array slices (`a[i:j]`); use arraySlice"));
                        }
                        AccessExpr::Dot(other) => {
                            return Err(refuse(format!("The field access .{other}")));
                        }
                    };
                }
                *e = out;
                self.expr(e)
            }
            Expr::IsFalse(_)
            | Expr::IsNotFalse(_)
            | Expr::IsTrue(_)
            | Expr::IsNotTrue(_)
            | Expr::IsUnknown(_)
            | Expr::IsNotUnknown(_) => Err(refuse("IS TRUE / IS FALSE / IS UNKNOWN")),
            Expr::IsDistinctFrom(..) | Expr::IsNotDistinctFrom(..) => {
                Err(refuse("IS [NOT] DISTINCT FROM"))
            }
            Expr::JsonAccess { .. } => Err(refuse("JSON access operators")),
            Expr::IsNormalized { .. } => Err(refuse("IS NORMALIZED")),
            Expr::InUnnest { .. } => Err(refuse("IN UNNEST")),
            Expr::SimilarTo { .. } => Err(refuse("SIMILAR TO")),
            Expr::RLike { .. } => Err(refuse("RLIKE / REGEXP")),
            Expr::AnyOp { .. } | Expr::AllOp { .. } => Err(refuse("ANY / ALL comparisons")),
            Expr::Convert { .. } => Err(refuse("CONVERT")),
            Expr::AtTimeZone { .. } => Err(refuse("AT TIME ZONE")),
            Expr::Ceil { .. } | Expr::Floor { .. } => Err(refuse("CEIL/FLOOR … TO")),
            Expr::Overlay { .. } => Err(refuse("OVERLAY")),
            Expr::Collate { .. } => Err(refuse("COLLATE")),
            Expr::Prefixed { .. } => Err(refuse("Prefixed literals")),
            Expr::TypedString(_) => Err(refuse("Typed string literals (DATE '…')")),
            Expr::GroupingSets(_) | Expr::Cube(_) | Expr::Rollup(_) => {
                Err(later("GROUPING SETS / CUBE / ROLLUP", 34))
            }
            Expr::Struct { .. } => Err(refuse("STRUCT literals")),
            Expr::Named { .. } => Err(refuse("Named expressions")),
            Expr::Dictionary(fields) => {
                let mut args = Vec::with_capacity(fields.len() * 2);
                for DictionaryField { key, value } in std::mem::take(fields) {
                    args.push(match key.quote_style {
                        Some('\'') => Expr::Value(Value::SingleQuotedString(key.value).into()),
                        _ => Expr::Identifier(key),
                    });
                    args.push(*value);
                }
                *e = call("map", args);
                self.expr(e)
            }
            Expr::Map(Map { entries }) => {
                let mut args = Vec::with_capacity(entries.len() * 2);
                for MapEntry { key, value } in std::mem::take(entries) {
                    args.push(*key);
                    args.push(*value);
                }
                *e = call("map", args);
                self.expr(e)
            }
            Expr::MatchAgainst { .. } => Err(refuse("MATCH … AGAINST")),
            Expr::Wildcard(_) | Expr::QualifiedWildcard(..) => Ok(()),
            Expr::OuterJoin(_) => Err(refuse("(+) outer joins")),
            Expr::Prior(_) => Err(refuse("PRIOR")),
            Expr::MemberOf(_) => Err(refuse("MEMBER OF")),
        }
    }
}

/// Whether `e` holds a `COLUMNS(…)` matcher (outside subqueries).
fn has_columns_matcher(e: &Expr) -> bool {
    struct Find(bool);
    impl crate::scope::Walk for Find {
        fn expr(&mut self, e: &mut Expr) -> Result<(), ChError> {
            if let Expr::Function(f) = e
                && f.name.to_string().eq_ignore_ascii_case("columns")
            {
                self.0 = true;
                return Ok(());
            }
            crate::scope::children(e, self)
        }
        fn query(&mut self, _q: &mut Query) -> Result<(), ChError> {
            Ok(())
        }
    }
    let mut f = Find(false);
    let _ = crate::scope::Walk::expr(&mut f, &mut e.clone());
    f.0
}

fn setting_text(v: &Expr) -> String {
    match v {
        Expr::Value(ValueWithSpan {
            value: Value::SingleQuotedString(s),
            ..
        }) => s.clone(),
        other => other.to_string(),
    }
}
