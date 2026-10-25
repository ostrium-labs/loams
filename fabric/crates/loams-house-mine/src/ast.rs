//! Features from sqlparser's ClickHouse AST.
//!
//! Operators are recorded under the ClickHouse function names the server's
//! own parser gives them (`a = b` is `function:equals`), so a statement mined
//! through this path and one mined through `EXPLAIN AST` ([`crate::explain`])
//! name the same features the same way.

use std::ops::ControlFlow;

use sqlparser::ast::{
    BinaryOperator, CreateTableOptions, DataType, Expr, FormatClause, FunctionArguments,
    GroupByExpr, GroupByWithModifier, JoinOperator, LimitClause, Query, Select, SetExpr,
    SetOperator, SetQuantifier, Statement, TableFactor, UnaryOperator, Visit, Visitor,
};
use sqlparser::dialect::ClickHouseDialect;
use sqlparser::parser::Parser;

use crate::features::Features;

/// Parses `text` as one or more statements and returns their features, or
/// `None` if sqlparser refuses it.
pub fn features(text: &str) -> Option<Features> {
    let masked = crate::lexical::mask_parameters(text);
    let statements = Parser::parse_sql(&ClickHouseDialect {}, &masked).ok()?;
    let mut collector = Collector::default();
    for statement in &statements {
        let _ = statement.visit(&mut collector);
    }
    Some(collector.features)
}

#[derive(Default)]
struct Collector {
    features: Features,
    /// The table factors that are `ARRAY JOIN` expressions, by address.
    /// sqlparser 0.63 reads `ARRAY JOIN f(a) AS x, g(b) AS y` as an array
    /// join of `f(a)` followed by comma joins of `g(b)`, so both look like
    /// table functions (HE0 Task 2 ruling R2.11).
    array_join_factors: std::collections::HashSet<usize>,
}

impl Collector {
    fn syntax(&mut self, form: &str) {
        self.features.add("syntax", form);
    }
}

impl Visitor for Collector {
    type Break = ();

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<()> {
        match statement {
            Statement::Insert(insert) => {
                if let Some(settings) = &insert.settings {
                    for s in settings {
                        self.features.add("setting", &s.key.value);
                    }
                }
                if let Some(format) = &insert.format_clause {
                    self.features.add("format", &format.ident.value);
                    self.syntax("INSERT FORMAT");
                }
                match insert.source.as_deref().map(|q| q.body.as_ref()) {
                    Some(SetExpr::Values(_)) => self.syntax("INSERT VALUES"),
                    Some(_) => self.syntax("INSERT SELECT"),
                    None => {}
                }
            }
            Statement::CreateTable(create) => {
                for column in &create.columns {
                    self.features.data_type(&column.data_type.to_string());
                    for option in &column.options {
                        use sqlparser::ast::ColumnOption;
                        match &option.option {
                            ColumnOption::Default(_) => self.syntax("column DEFAULT"),
                            ColumnOption::Materialized(_) => self.syntax("column MATERIALIZED"),
                            ColumnOption::Alias(_) => self.syntax("column ALIAS"),
                            ColumnOption::Ephemeral(_) => self.syntax("column EPHEMERAL"),
                            ColumnOption::Comment(_) => self.syntax("column COMMENT"),
                            _ => {}
                        }
                    }
                }
                self.engine(&create.table_options);
                if create.on_cluster.is_some() {
                    self.syntax("ON CLUSTER");
                }
                if create.partition_by.is_some() {
                    self.syntax("PARTITION BY");
                }
                if create.order_by.is_some() {
                    self.syntax("table ORDER BY");
                }
                if create.primary_key.is_some() {
                    self.syntax("table PRIMARY KEY");
                }
                if create.query.is_some() {
                    self.syntax("CREATE TABLE AS SELECT");
                }
            }
            Statement::CreateView(view) => {
                if view.to.is_some() {
                    self.syntax("MATERIALIZED VIEW TO");
                }
            }
            Statement::OptimizeTable {
                on_cluster,
                include_final,
                deduplicate,
                ..
            } => {
                if on_cluster.is_some() {
                    self.syntax("ON CLUSTER");
                }
                if *include_final {
                    self.syntax("OPTIMIZE FINAL");
                }
                if deduplicate.is_some() {
                    self.syntax("OPTIMIZE DEDUPLICATE");
                }
            }
            Statement::Set(set) => {
                for name in set_names(set) {
                    self.features.add("setting", &name);
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        if query.with.is_some() {
            self.syntax("WITH");
        }
        if let Some(settings) = &query.settings {
            for s in settings {
                self.features.add("setting", &s.key.value);
            }
        }
        if let Some(FormatClause::Identifier(format)) = &query.format_clause {
            self.features.add("format", &format.value);
        }
        if let Some(order_by) = &query.order_by {
            self.syntax("ORDER BY");
            if order_by.interpolate.is_some() {
                self.syntax("INTERPOLATE");
            }
            if let sqlparser::ast::OrderByKind::Expressions(exprs) = &order_by.kind
                && exprs.iter().any(|e| e.with_fill.is_some())
            {
                self.syntax("WITH FILL");
            }
        }
        match &query.limit_clause {
            Some(LimitClause::LimitOffset {
                limit,
                offset,
                limit_by,
            }) => {
                if limit.is_some() {
                    self.syntax("LIMIT");
                }
                if offset.is_some() {
                    self.syntax("OFFSET");
                }
                if !limit_by.is_empty() {
                    self.syntax("LIMIT BY");
                }
            }
            Some(LimitClause::OffsetCommaLimit { .. }) => self.syntax("LIMIT"),
            None => {}
        }
        self.set_expr(&query.body);
        ControlFlow::Continue(())
    }

    fn pre_visit_select(&mut self, select: &Select) -> ControlFlow<()> {
        if select.distinct.is_some() {
            self.syntax("DISTINCT");
        }
        if select.prewhere.is_some() {
            self.syntax("PREWHERE");
        }
        if select.having.is_some() {
            self.syntax("HAVING");
        }
        if !select.named_window.is_empty() {
            self.syntax("WINDOW");
        }
        match &select.group_by {
            GroupByExpr::All(modifiers) => {
                self.syntax("GROUP BY ALL");
                self.group_by_modifiers(modifiers);
            }
            GroupByExpr::Expressions(exprs, modifiers) => {
                if !exprs.is_empty() {
                    self.syntax("GROUP BY");
                }
                self.group_by_modifiers(modifiers);
            }
        }
        let mut after_array_join = false;
        for table in &select.from {
            if after_array_join {
                self.array_join_factors
                    .insert(std::ptr::from_ref(&table.relation) as usize);
            }
            for join in &table.joins {
                if matches!(
                    join.join_operator,
                    JoinOperator::ArrayJoin
                        | JoinOperator::LeftArrayJoin
                        | JoinOperator::InnerArrayJoin
                ) {
                    after_array_join = true;
                    self.array_join_factors
                        .insert(std::ptr::from_ref(&join.relation) as usize);
                }
                let form = match &join.join_operator {
                    JoinOperator::ArrayJoin => "ARRAY JOIN",
                    JoinOperator::LeftArrayJoin => "LEFT ARRAY JOIN",
                    JoinOperator::InnerArrayJoin => "INNER ARRAY JOIN",
                    JoinOperator::Join(_) | JoinOperator::Inner(_) => "INNER JOIN",
                    JoinOperator::Left(_) | JoinOperator::LeftOuter(_) => "LEFT JOIN",
                    JoinOperator::Right(_) | JoinOperator::RightOuter(_) => "RIGHT JOIN",
                    JoinOperator::FullOuter(_) => "FULL JOIN",
                    JoinOperator::CrossJoin(_) => "CROSS JOIN",
                    JoinOperator::Semi(_)
                    | JoinOperator::LeftSemi(_)
                    | JoinOperator::RightSemi(_) => "SEMI JOIN",
                    JoinOperator::Anti(_)
                    | JoinOperator::LeftAnti(_)
                    | JoinOperator::RightAnti(_) => "ANTI JOIN",
                    JoinOperator::AsOf { .. } => "ASOF JOIN",
                    _ => "JOIN (other)",
                };
                self.syntax(form);
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, factor: &TableFactor) -> ControlFlow<()> {
        if self
            .array_join_factors
            .contains(&(std::ptr::from_ref(factor) as usize))
        {
            // An `ARRAY JOIN` expression: a call is a function, not a table
            // function.
            let name = match factor {
                TableFactor::Table {
                    name,
                    args: Some(_),
                    ..
                }
                | TableFactor::Function { name, .. } => name.0.last().map(|l| {
                    l.as_ident()
                        .map_or_else(|| l.to_string(), |i| i.value.clone())
                }),
                _ => None,
            };
            if let Some(n) = name {
                self.features.function(&n);
            }
            return ControlFlow::Continue(());
        }
        match factor {
            TableFactor::Table {
                name,
                alias,
                args,
                sample,
                ..
            } => {
                // sqlparser 0.63 reads `FROM t FINAL` as the alias `FINAL`
                // (HE0 Task 2 ruling).
                if alias.as_ref().is_some_and(|a| {
                    a.name.quote_style.is_none() && a.name.value.eq_ignore_ascii_case("final")
                }) {
                    self.syntax("FINAL");
                }
                if sample.is_some() {
                    self.syntax("SAMPLE");
                }
                if args.is_some()
                    && let Some(last) = name.0.last()
                {
                    let n = last
                        .as_ident()
                        .map_or_else(|| last.to_string(), |i| i.value.clone());
                    self.features.add("table_function", &n);
                }
            }
            TableFactor::Function { name, .. } => {
                if let Some(last) = name.0.last() {
                    let n = last
                        .as_ident()
                        .map_or_else(|| last.to_string(), |i| i.value.clone());
                    self.features.add("table_function", &n);
                }
            }
            TableFactor::Derived { .. } => self.syntax("subquery in FROM"),
            _ => {}
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        let f = &mut self.features;
        match expr {
            Expr::Function(func) => {
                if let Some(last) = func.name.0.last() {
                    let n = last
                        .as_ident()
                        .map_or_else(|| last.to_string(), |i| i.value.clone());
                    if n == "_CAST" {
                        // `query_log` holds a query's text after the server bound
                        // its parameters: `{p:UUID}` is logged as
                        // `_CAST('…', 'UUID')` (HE0 Task 5 ruling).
                        f.add("syntax", "query parameter");
                        if let Some(ty) = string_arg(&func.args, 1) {
                            f.data_type(&ty);
                        }
                    } else {
                        f.function(&n);
                    }
                }
                if !matches!(func.parameters, FunctionArguments::None) {
                    f.add("syntax", "parametric function");
                }
                if func.over.is_some() {
                    f.add("syntax", "window function");
                }
                if let FunctionArguments::List(list) = &func.args
                    && list.duplicate_treatment.is_some()
                {
                    f.add("syntax", "aggregate DISTINCT");
                }
            }
            Expr::BinaryOp { op, .. } => {
                if let Some(name) = binary_function(op) {
                    f.function(name);
                }
            }
            Expr::UnaryOp { op, .. } => match op {
                UnaryOperator::Not => f.function("not"),
                UnaryOperator::Minus => f.function("negate"),
                _ => {}
            },
            Expr::IsNull(_) => f.function("isNull"),
            Expr::IsNotNull(_) => f.function("isNotNull"),
            Expr::InList { negated, .. } => f.function(if *negated { "notIn" } else { "in" }),
            Expr::InSubquery { negated, .. } => {
                f.function(if *negated { "notIn" } else { "in" });
                f.add("syntax", "IN subquery");
            }
            Expr::Between { negated, .. } => {
                f.add("syntax", "BETWEEN");
                if *negated {
                    f.function("less");
                    f.function("greater");
                    f.function("or");
                } else {
                    f.function("greaterOrEquals");
                    f.function("lessOrEquals");
                    f.function("and");
                }
            }
            Expr::Like { negated, .. } => f.function(if *negated { "notLike" } else { "like" }),
            Expr::ILike { negated, .. } => f.function(if *negated { "notILike" } else { "ilike" }),
            Expr::Cast {
                data_type, kind, ..
            } => {
                f.function("CAST");
                if matches!(kind, sqlparser::ast::CastKind::DoubleColon) {
                    f.add("syntax", "cast operator ::");
                }
                data_type_features(f, data_type);
            }
            Expr::Case { operand, .. } => {
                f.function(if operand.is_some() {
                    "caseWithExpression"
                } else {
                    "multiIf"
                });
            }
            Expr::Array(_) => f.function("array"),
            Expr::Tuple(_) => f.function("tuple"),
            Expr::Subquery(_) => f.add("syntax", "scalar subquery"),
            Expr::Exists { .. } => f.function("exists"),
            Expr::Lambda(_) => f.function("lambda"),
            Expr::Interval(interval) => {
                if let Some(unit) = &interval.leading_field {
                    f.function(&format!("toInterval{}", interval_unit(&unit.to_string())));
                }
            }
            Expr::Extract { .. } => f.add("syntax", "EXTRACT"),
            Expr::Substring { .. } => f.function("substring"),
            Expr::Trim { .. } => f.function("trim"),
            Expr::Position { .. } => f.function("position"),
            Expr::Ceil { .. } => f.function("ceil"),
            Expr::Floor { .. } => f.function("floor"),
            Expr::CompoundFieldAccess { .. } => f.function("arrayElement"),
            Expr::TypedString(ts) => data_type_features(f, &ts.data_type),
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

impl Collector {
    fn engine(&mut self, options: &CreateTableOptions) {
        let list = match options {
            CreateTableOptions::Plain(l)
            | CreateTableOptions::With(l)
            | CreateTableOptions::Options(l)
            | CreateTableOptions::TableProperties(l) => l,
            CreateTableOptions::None => return,
        };
        for option in list {
            let text = option.to_string();
            let upper = text.to_ascii_uppercase();
            if let Some(rest) = upper.strip_prefix("ENGINE") {
                let offset = text.len() - rest.len();
                let name: String = text[offset..]
                    .trim_start_matches([' ', '='])
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                self.features.add("engine", &name);
            } else if upper.starts_with("SETTINGS") {
                self.syntax("table SETTINGS");
            }
        }
    }

    fn group_by_modifiers(&mut self, modifiers: &[GroupByWithModifier]) {
        for m in modifiers {
            match m {
                GroupByWithModifier::Totals => self.syntax("WITH TOTALS"),
                GroupByWithModifier::Rollup => self.syntax("WITH ROLLUP"),
                GroupByWithModifier::Cube => self.syntax("WITH CUBE"),
                GroupByWithModifier::GroupingSets(_) => self.syntax("GROUPING SETS"),
            }
        }
    }

    fn set_expr(&mut self, body: &SetExpr) {
        if let SetExpr::SetOperation {
            op, set_quantifier, ..
        } = body
        {
            let form = match (op, set_quantifier) {
                (SetOperator::Union, SetQuantifier::All) => "UNION ALL",
                (SetOperator::Union, SetQuantifier::Distinct) => "UNION DISTINCT",
                (SetOperator::Union, _) => "UNION",
                (SetOperator::Except, _) => "EXCEPT",
                (SetOperator::Intersect, _) => "INTERSECT",
                (SetOperator::Minus, _) => "MINUS",
            };
            self.syntax(form);
        }
    }
}

/// The `index`th argument of a call, when it is a string literal.
fn string_arg(args: &FunctionArguments, index: usize) -> Option<String> {
    use sqlparser::ast::{FunctionArg, FunctionArgExpr, Value};
    let FunctionArguments::List(list) = args else {
        return None;
    };
    match list.args.get(index)? {
        FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(v))) => match &v.value {
            Value::SingleQuotedString(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn data_type_features(f: &mut Features, data_type: &DataType) {
    f.data_type(&data_type.to_string());
}

/// `SET a = 1, b = 2`'s names.
fn set_names(set: &sqlparser::ast::Set) -> Vec<String> {
    use sqlparser::ast::Set;
    match set {
        Set::SingleAssignment { variable, .. } => vec![variable.to_string()],
        Set::MultipleAssignments { assignments } => {
            assignments.iter().map(|a| a.name.to_string()).collect()
        }
        _ => Vec::new(),
    }
}

/// The ClickHouse function an operator is parsed into.
fn binary_function(op: &BinaryOperator) -> Option<&'static str> {
    Some(match op {
        BinaryOperator::Plus => "plus",
        BinaryOperator::Minus => "minus",
        BinaryOperator::Multiply => "multiply",
        BinaryOperator::Divide => "divide",
        BinaryOperator::Modulo => "modulo",
        BinaryOperator::StringConcat => "concat",
        BinaryOperator::Gt => "greater",
        BinaryOperator::Lt => "less",
        BinaryOperator::GtEq => "greaterOrEquals",
        BinaryOperator::LtEq => "lessOrEquals",
        BinaryOperator::Eq => "equals",
        BinaryOperator::NotEq => "notEquals",
        BinaryOperator::And => "and",
        BinaryOperator::Or => "or",
        BinaryOperator::DuckIntegerDivide | BinaryOperator::MyIntegerDivide => "intDiv",
        _ => return None,
    })
}

/// `DAY` → `Day`, for `toIntervalDay` and `toDay`-style names.
fn interval_unit(unit: &str) -> String {
    let lower = unit.trim().to_ascii_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}
