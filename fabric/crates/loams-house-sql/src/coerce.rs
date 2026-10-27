//! ClickHouse's type rules on DataFusion plans (plan HE1 Task 24).
//!
//! [`ClickHouseCoercion`] is an `AnalyzerRule` that runs before DataFusion's
//! `TypeCoercion` and gives every expression ClickHouse's type and value:
//!
//! * **literals** take ClickHouse's types: `1` is `UInt8`, `-1` is `Int8`,
//!   `256` is `UInt16` ([`literal::narrow`]);
//! * **arithmetic** widens as ClickHouse does (`UInt8 + UInt8` is `UInt16`,
//!   `UInt8 - UInt8` is `Int16`), `/` is always `Float64`, `%` follows
//!   ClickHouse's sign and width rules, and integers wrap: each operator stays
//!   a DataFusion `BinaryExpr` over operands cast to the result type, whose
//!   kernels wrap (`add_wrapping`), so pruning and pushdown keep working;
//! * **comparisons** compare exactly across signed and unsigned integers,
//!   read a string constant as the other operand's type (`d = '2026-10-01'`,
//!   an `Enum` name, a UUID or an IP), and compare `Date` with `DateTime`;
//! * **supertypes** ([`rules::least_supertype`]) for `CASE`, `if`,
//!   `multiIf`, `coalesce`, array literals and `UNION` branches;
//! * **boolean contexts**: `WHERE`, `HAVING`, `AND`, `OR` and `NOT` take any
//!   number (`x` means `x != 0`); comparisons and logical operators are
//!   `UInt8` values (`Bool` when an operand is a `Bool`), carried as Arrow
//!   booleans inside a predicate and cast to `UInt8` wherever they become a
//!   value;
//! * **`Nullable`** follows Arrow's nullability.
//!
//! **How a type is carried (R24.1).** After this rule, every expression's
//! ClickHouse type is its Arrow field's: the `loams.ch.type` metadata when
//! present, else the type inferred from Arrow. Where Arrow alone is ambiguous
//! (a `Date` is `Date32`, an `Enum8` is `Int8`), the expression is wrapped in
//! a cast to its own Arrow type whose target field carries the ClickHouse type
//! ([`tag`]). The one exception is a predicate (a comparison, `AND`, …): its
//! Arrow type is `Boolean` and its ClickHouse type `UInt8`, decided by
//! [`HouseTypes::ch_type`] from the expression's kind, and cast to `UInt8`
//! when it is used as a value ([`HouseTypes::value`]).
//!
//! DataFusion's `TypeCoercion` still runs afterwards, for what this rule
//! leaves alone (function signatures); on the operators this rule has typed,
//! both operands already have one type, so it changes nothing there.

mod literal;
pub mod plan_time;
pub mod rules;
pub mod wrap;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};
use datafusion::common::config::ConfigOptions;
use datafusion::common::tree_node::{Transformed, TreeNode, TreeNodeRewriter};
use datafusion::common::{DFSchema, Result, ScalarValue};
use datafusion::logical_expr::expr::{
    AggregateFunction, AggregateFunctionParams, Alias, Between, BinaryExpr, Case, Cast, InList,
    ScalarFunction, WindowFunction, WindowFunctionParams,
};
use datafusion::logical_expr::expr_rewriter::NamePreserver;
use datafusion::logical_expr::utils::merge_schema;
use datafusion::logical_expr::{
    Aggregate, Expr, ExprSchemable, Filter, Join, LogicalPlan, Operator, Projection, Union,
};
use datafusion::optimizer::AnalyzerRule;
use datafusion::optimizer::analyzer::resolve_grouping_function::ResolveGroupingFunction;
use datafusion::optimizer::analyzer::type_coercion::TypeCoercion;
use loams_house_types::arrow::{CH_TYPE, data_type, from_arrow, nullable};
use loams_house_types::{ChError, ChType};

use crate::context::external;
use rules::Arith;

pub use plan_time::{COND, ClickHouseOperators, mark_conditions};

/// The engine's analyzer rules: DataFusion's defaults with
/// [`ClickHouseCoercion`] before `TypeCoercion`.
pub fn analyzer_rules() -> Vec<Arc<dyn AnalyzerRule + Send + Sync>> {
    vec![
        Arc::new(ResolveGroupingFunction::new()),
        Arc::new(ClickHouseCoercion),
        Arc::new(TypeCoercion::new()),
    ]
}

/// ClickHouse's type rules (see the module documentation).
#[derive(Debug, Default)]
pub struct ClickHouseCoercion;

impl AnalyzerRule for ClickHouseCoercion {
    fn name(&self) -> &str {
        "clickhouse_type_rules"
    }

    fn analyze(&self, plan: LogicalPlan, _config: &ConfigOptions) -> Result<LogicalPlan> {
        Ok(plan.transform_up_with_subqueries(analyze_node)?.data)
    }
}

/// The ClickHouse type text of an Arrow type, for messages.
pub fn ch_text(dt: &DataType) -> String {
    from_arrow(&Field::new("", dt.clone(), false))
        .map(|t| t.to_string())
        .unwrap_or_else(|_| dt.to_string())
}

fn ch(e: ChError) -> datafusion::common::DataFusionError {
    external(e)
}

fn analyze_node(plan: LogicalPlan) -> Result<Transformed<LogicalPlan>> {
    static EMPTY: LazyLock<DFSchema> = LazyLock::new(DFSchema::empty);
    let mut schema = merge_schema(&plan.inputs());
    if let LogicalPlan::TableScan(ts) = &plan {
        let source =
            DFSchema::try_from_qualified_schema(ts.table_name.clone(), &ts.source.schema())?;
        schema.merge(&source);
    }
    schema.merge(&EMPTY);
    let types = HouseTypes { schema: &schema };
    let preserver = NamePreserver::new(&plan);
    let rewrite = |e: Expr| -> Result<Expr> { Ok(e.rewrite(&mut types.rewriter())?.data) };
    let plan = match plan {
        LogicalPlan::Union(union) => return coerce_union(union).map(Transformed::yes),
        LogicalPlan::Filter(Filter {
            predicate, input, ..
        }) => {
            let p = rewrite(predicate)?;
            let p = types.predicate(p, Context::Filter)?;
            LogicalPlan::Filter(Filter::try_new(p, input)?)
        }
        LogicalPlan::Projection(Projection { expr, input, .. }) => {
            let mut out = Vec::with_capacity(expr.len());
            for e in expr {
                let saved = preserver.save(&e);
                let e = types.settle(rewrite(e)?)?;
                out.push(saved.restore(e));
            }
            LogicalPlan::Projection(Projection::try_new(out, input)?)
        }
        LogicalPlan::Aggregate(Aggregate {
            input,
            group_expr,
            aggr_expr,
            ..
        }) => {
            let mut groups = Vec::with_capacity(group_expr.len());
            for e in group_expr {
                let saved = preserver.save(&e);
                let e = match e {
                    // Grouping sets keep their shape.
                    Expr::GroupingSet(_) => rewrite(e)?,
                    other => types.settle(rewrite(other)?)?,
                };
                groups.push(saved.restore(e));
            }
            let mut aggrs = Vec::with_capacity(aggr_expr.len());
            for e in aggr_expr {
                let saved = preserver.save(&e);
                aggrs.push(saved.restore(rewrite(e)?));
            }
            LogicalPlan::Aggregate(Aggregate::try_new(input, groups, aggrs)?)
        }
        LogicalPlan::Join(mut join) => {
            join.filter = match join.filter.take() {
                Some(f) => Some(types.predicate(rewrite(f)?, Context::Filter)?),
                None => None,
            };
            let on = std::mem::take(&mut join.on);
            let mut keys = Vec::with_capacity(on.len());
            for (l, r) in on {
                keys.push((rewrite(l)?, rewrite(r)?));
            }
            join.on = keys;
            LogicalPlan::Join(Join { ..join }).recompute_schema()?
        }
        other => other
            .map_expressions(|e| {
                let saved = preserver.save(&e);
                Ok(Transformed::yes(saved.restore(rewrite(e)?)))
            })?
            .data
            .recompute_schema()?,
    };
    Ok(Transformed::yes(plan))
}

/// The branches of a `UNION` converted to their columns' least supertypes.
fn coerce_union(union: Union) -> Result<LogicalPlan> {
    let width = union.schema.fields().len();
    let mut supertypes = Vec::with_capacity(width);
    for i in 0..width {
        let mut column = Vec::with_capacity(union.inputs.len());
        for input in &union.inputs {
            column.push(field_type(input.schema().field(i)));
        }
        supertypes.push(rules::least_supertype(&column).map_err(ch)?);
    }
    let mut inputs = Vec::with_capacity(union.inputs.len());
    for input in union.inputs {
        let schema = input.schema().clone();
        let types = HouseTypes { schema: &schema };
        let needs = (0..width).any(|i| field_type(schema.field(i)) != supertypes[i]);
        if !needs {
            inputs.push(input);
            continue;
        }
        let mut exprs = Vec::with_capacity(width);
        for (i, (qualifier, field)) in schema.iter().enumerate() {
            let col = Expr::Column(datafusion::common::Column::from((qualifier, field)));
            let e = types.cast_to(col, &supertypes[i])?;
            exprs.push(e.alias_qualified(qualifier.cloned(), field.name()));
        }
        inputs.push(Arc::new(LogicalPlan::Projection(Projection::try_new(
            exprs, input,
        )?)));
    }
    Ok(LogicalPlan::Union(Union::try_new_with_loose_types(inputs)?))
}

/// The ClickHouse type of a field: its metadata's when it matches the Arrow
/// type, else the type inferred from Arrow; `Nullable` when the field is.
fn field_type(field: &Field) -> ChType {
    let inferred = || {
        from_arrow(&Field::new(
            "",
            field.data_type().clone(),
            field.is_nullable(),
        ))
        .unwrap_or(ChType::Nothing)
    };
    let t = match field.metadata().get(CH_TYPE) {
        Some(text) => match loams_house_types::parse(text) {
            Ok(t) if data_type(&t).is_ok_and(|(dt, _)| dt == *field.data_type()) => t,
            _ => return inferred(),
        },
        None => return inferred(),
    };
    if field.is_nullable() && !t.is_nullable() {
        nullable(t)
    } else {
        t
    }
}

/// A `CAST` to `t`'s Arrow type whose field carries `t`, so the cast's
/// result has ClickHouse type `t` (R24.1).
pub fn tag(e: Expr, t: &ChType) -> Result<Expr> {
    let (dt, _) = data_type(t).map_err(ch)?;
    let mut metadata = HashMap::new();
    metadata.insert(CH_TYPE.to_string(), t.to_compact_string());
    Ok(Expr::Cast(Cast {
        expr: Box::new(e),
        field: Arc::new(Field::new("", dt, true).with_metadata(metadata)),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    Filter,
    Function(&'static str),
}

/// ClickHouse types of expressions over one input schema.
struct HouseTypes<'a> {
    schema: &'a DFSchema,
}

/// Whether DataFusion types `e` as a predicate (a `Boolean` that is a
/// ClickHouse `UInt8`).
fn is_predicate(e: &Expr) -> bool {
    match e {
        Expr::BinaryExpr(BinaryExpr { op, .. }) => matches!(
            op,
            Operator::Eq
                | Operator::NotEq
                | Operator::Lt
                | Operator::LtEq
                | Operator::Gt
                | Operator::GtEq
                | Operator::And
                | Operator::Or
                | Operator::IsDistinctFrom
                | Operator::IsNotDistinctFrom
                | Operator::RegexMatch
                | Operator::RegexIMatch
                | Operator::RegexNotMatch
                | Operator::RegexNotIMatch
                | Operator::LikeMatch
                | Operator::ILikeMatch
                | Operator::NotLikeMatch
                | Operator::NotILikeMatch
        ),
        Expr::Not(_)
        | Expr::IsNull(_)
        | Expr::IsNotNull(_)
        | Expr::IsTrue(_)
        | Expr::IsFalse(_)
        | Expr::IsUnknown(_)
        | Expr::IsNotTrue(_)
        | Expr::IsNotFalse(_)
        | Expr::IsNotUnknown(_)
        | Expr::Like(_)
        | Expr::SimilarTo(_)
        | Expr::Between(_)
        | Expr::InList(_)
        | Expr::InSubquery(_)
        | Expr::Exists(_)
        | Expr::SetComparison(_) => true,
        Expr::Alias(Alias { expr, .. }) => is_predicate(expr),
        _ => false,
    }
}

fn base(t: &ChType) -> ChType {
    match t {
        ChType::Nullable(x) | ChType::LowCardinality(x) => base(x),
        other => other.clone(),
    }
}

fn is_string_type(t: &ChType) -> bool {
    matches!(t, ChType::String | ChType::FixedString(_))
}

/// A constant string: a literal, or a `CAST` of one to `String` (a bound
/// parameter).
fn string_constant(e: &Expr) -> Option<&str> {
    match e {
        Expr::Literal(ScalarValue::Utf8(Some(s)) | ScalarValue::LargeUtf8(Some(s)), _) => Some(s),
        Expr::Cast(Cast { expr, field }) if field.data_type() == &DataType::Utf8 => {
            string_constant(expr)
        }
        _ => None,
    }
}

fn arith_of(op: Operator) -> Option<Arith> {
    Some(match op {
        Operator::Plus => Arith::Plus,
        Operator::Minus => Arith::Minus,
        Operator::Multiply => Arith::Multiply,
        Operator::Divide => Arith::Divide,
        Operator::Modulo => Arith::Modulo,
        _ => return None,
    })
}

fn is_comparison(op: Operator) -> bool {
    matches!(
        op,
        Operator::Eq
            | Operator::NotEq
            | Operator::Lt
            | Operator::LtEq
            | Operator::Gt
            | Operator::GtEq
    )
}

fn binary(l: Expr, op: Operator, r: Expr) -> Expr {
    Expr::BinaryExpr(BinaryExpr::new(Box::new(l), op, Box::new(r)))
}

fn lit(v: ScalarValue) -> Expr {
    Expr::Literal(v, None)
}

/// Functions whose value arguments take their least supertype, with the
/// positions of those arguments.
fn supertype_arguments(name: &str, n: usize) -> Option<Vec<usize>> {
    match name {
        "if" if n == 3 => Some(vec![1, 2]),
        "multiIf" if n >= 3 && n % 2 == 1 => {
            Some((0..n).filter(|i| i % 2 == 1 || *i == n - 1).collect())
        }
        "array" | "make_array" | "greatest" | "least" => Some((0..n).collect()),
        _ => None,
    }
}

impl HouseTypes<'_> {
    fn rewriter(&self) -> Rewriter<'_, '_> {
        Rewriter { types: self }
    }

    fn field(&self, e: &Expr) -> Result<Arc<Field>> {
        Ok(e.to_field(self.schema)?.1)
    }

    /// The ClickHouse type of `e` (already rewritten).
    pub fn ch_type(&self, e: &Expr) -> Result<ChType> {
        let field = self.field(e)?;
        if is_predicate(e) && field.data_type() == &DataType::Boolean {
            // `and`, `or` and `not` are `Bool` when an argument is exactly a
            // `Bool` (FunctionsLogical.cpp); every other predicate is UInt8.
            let any_bool = match e {
                Expr::BinaryExpr(BinaryExpr {
                    left,
                    op: Operator::And | Operator::Or,
                    right,
                }) => self.ch_type(left)? == ChType::Bool || self.ch_type(right)? == ChType::Bool,
                Expr::Not(x) => self.ch_type(x)? == ChType::Bool,
                _ => false,
            };
            let t = if any_bool {
                ChType::Bool
            } else {
                ChType::UInt8
            };
            return Ok(if field.is_nullable() { nullable(t) } else { t });
        }
        Ok(field_type(&field))
    }

    /// `e` cast to ClickHouse type `t` (the cast tagged with `t`); `e` itself
    /// when it already is one.
    fn cast_to(&self, e: Expr, t: &ChType) -> Result<Expr> {
        let current = self.ch_type(&e)?;
        if current == *t {
            return Ok(e);
        }
        let (dt, _) = data_type(t).map_err(ch)?;
        let field = self.field(&e)?;
        let e = if field.data_type() == &dt {
            e
        } else {
            Expr::Cast(Cast::new(Box::new(e), dt))
        };
        tag(e, t)
    }

    /// `e` cast to the Arrow type of ClickHouse type `t`, for computing; the
    /// ClickHouse type is decided by the caller.
    fn compute_as(&self, e: Expr, t: &ChType) -> Result<Expr> {
        let (dt, _) = data_type(&base(t)).map_err(ch)?;
        let from = self.field(&e)?.data_type().clone();
        if from == dt {
            return Ok(e);
        }
        // ClickHouse's integer conversions wrap (`wrap`).
        Ok(wrap::convert(e, &from, &dt))
    }

    /// A predicate used as a value: cast to `UInt8`.
    pub fn value(&self, e: Expr) -> Result<Expr> {
        if self.field(&e)?.data_type() != &DataType::Boolean {
            return Ok(e);
        }
        let t = self.ch_type(&e)?;
        if base(&t) == ChType::UInt8 {
            let target = if t.is_nullable() {
                nullable(ChType::UInt8)
            } else {
                ChType::UInt8
            };
            return tag(Expr::Cast(Cast::new(Box::new(e), DataType::UInt8)), &target);
        }
        Ok(e)
    }

    /// `e` as an output value: a predicate cast to `UInt8`, and the type
    /// tagged where Arrow alone would read another one.
    fn settle(&self, e: Expr) -> Result<Expr> {
        if let Expr::Alias(Alias {
            expr,
            relation,
            name,
            metadata,
        }) = e
        {
            return Ok(Expr::Alias(Alias {
                expr: Box::new(self.settle(*expr)?),
                relation,
                name,
                metadata,
            }));
        }
        if matches!(e, Expr::Column(_)) {
            return Ok(e);
        }
        let e = self.value(e)?;
        let t = self.ch_type(&e)?;
        let field = self.field(&e)?;
        let read = from_arrow(&field).ok();
        if read.as_ref() == Some(&t) || data_type(&t).is_err() {
            return Ok(e);
        }
        tag(e, &t)
    }

    /// `e` where a condition is expected: a boolean as it is, a number `x`
    /// as `x != 0`, `NULL` as a `NULL` boolean.
    fn predicate(&self, e: Expr, context: Context) -> Result<Expr> {
        // A predicate this rule cast to UInt8 for a value context.
        if let Expr::Cast(Cast { expr, field }) = &e
            && field.data_type() == &DataType::UInt8
            && let Expr::Cast(Cast { expr: inner, .. }) = expr.as_ref()
            && self.field(inner)?.data_type() == &DataType::Boolean
        {
            return Ok(inner.as_ref().clone());
        }
        let dt = self.field(&e)?.data_type().clone();
        match &dt {
            DataType::Boolean => Ok(e),
            DataType::Null => tag(
                Expr::Cast(Cast::new(Box::new(e), DataType::Boolean)),
                &nullable(ChType::UInt8),
            ),
            _ => match context {
                Context::Function(f) => plan_time::nonzero(e, &dt, f),
                Context::Filter => plan_time::nonzero(e, &dt, "filter").map_err(|_| {
                    ch(rules::illegal_filter(format!(
                        "Invalid type for filter in WHERE: {}",
                        ch_text(&dt)
                    )))
                }),
            },
        }
    }

    /// A string constant in a comparison with a value of type `other`,
    /// read as that type.
    fn constant_as(&self, e: Expr, other: &ChType) -> Result<Result<Expr, Expr>> {
        let Some(s) = string_constant(&e) else {
            return Ok(Err(e));
        };
        let target = base(other);
        if is_string_type(&target) {
            return Ok(Err(e));
        }
        match literal::convert(s, &target).map_err(ch)? {
            Some((v, t)) => {
                let mut metadata = HashMap::new();
                metadata.insert(CH_TYPE.to_string(), t.to_compact_string());
                Ok(Ok(Expr::Literal(v, Some(metadata.into()))))
            }
            None => Ok(Err(e)),
        }
    }

    /// An `Enum` value as its element's name (`CASE e WHEN 1 THEN 'a' …`).
    fn enum_name(&self, e: Expr, t: &ChType) -> Result<Expr> {
        let pairs: Vec<(ScalarValue, String)> = match base(t) {
            ChType::Enum8(v) => v
                .into_iter()
                .map(|(n, v)| (ScalarValue::Int8(Some(v)), n))
                .collect(),
            ChType::Enum16(v) => v
                .into_iter()
                .map(|(n, v)| (ScalarValue::Int16(Some(v)), n))
                .collect(),
            _ => return Ok(e),
        };
        Ok(Expr::Case(Case {
            expr: Some(Box::new(e)),
            when_then_expr: pairs
                .into_iter()
                .map(|(v, n)| (Box::new(lit(v)), Box::new(lit(ScalarValue::Utf8(Some(n))))))
                .collect(),
            else_expr: None,
        }))
    }

    /// `l op r` for a comparison operator.
    fn compare(&self, l: Expr, op: Operator, r: Expr) -> Result<Expr> {
        let (l, r) = (self.value(l)?, self.value(r)?);
        let (lt, rt) = (self.ch_type(&l)?, self.ch_type(&r)?);
        let (mut l, mut r) = (l, r);
        let mut by_name = false;
        match self.constant_as(r, &lt)? {
            Ok(x) => r = x,
            Err(x) => {
                by_name = rules::is_enum(&base(&lt)) && string_constant(&x).is_some();
                r = x;
            }
        }
        match self.constant_as(l, &rt)? {
            Ok(x) => l = x,
            Err(x) => {
                by_name |= rules::is_enum(&base(&rt)) && string_constant(&x).is_some();
                l = x;
            }
        }
        let (mut lb, mut rb) = (base(&self.ch_type(&l)?), base(&self.ch_type(&r)?));
        // An Enum against a string compares by name.
        if by_name
            || rules::is_enum(&lb) && is_string_type(&rb)
            || rules::is_enum(&rb) && is_string_type(&lb)
        {
            if rules::is_enum(&lb) {
                l = self.enum_name(l, &lb)?;
                lb = ChType::String;
            }
            if rules::is_enum(&rb) {
                r = self.enum_name(r, &rb)?;
                rb = ChType::String;
            }
        }
        // Strings (and LowCardinality strings) compare as they are.
        if is_string_type(&lb) && is_string_type(&rb) {
            return Ok(binary(l, op, r));
        }
        let t = rules::comparison(&lb, &rb).map_err(ch)?;
        let l = self.compute_as(l, &t)?;
        let r = self.compute_as(r, &t)?;
        Ok(binary(l, op, r))
    }

    /// `l op r` for an arithmetic operator.
    fn arithmetic(&self, l: Expr, op: Arith, r: Expr) -> Result<Expr> {
        let (l, r) = (self.value(l)?, self.value(r)?);
        let (lt, rt) = (self.ch_type(&l)?, self.ch_type(&r)?);
        let nullable_result = lt.is_nullable() || rt.is_nullable();
        let (lb, rb) = (base(&lt), base(&rt));
        let operator = match op {
            Arith::Plus => Operator::Plus,
            Arith::Minus => Operator::Minus,
            Arith::Multiply => Operator::Multiply,
            Arith::Divide => Operator::Divide,
            Arith::Modulo => Operator::Modulo,
            Arith::IntDiv => unreachable!("intDiv is a function"),
        };
        // `NULL + 1` is `Nullable(Nothing)`.
        if lb == ChType::Nothing || rb == ChType::Nothing {
            return tag(
                lit(ScalarValue::Null),
                &ChType::Nullable(Box::new(ChType::Nothing)),
            );
        }
        if let Some(result) = rules::date_arithmetic(op, &lb, &rb) {
            let result = result.map_err(ch)?;
            return self.date_arithmetic(l, &lb, op, r, &rb, &result);
        }
        let result = rules::arithmetic(op, &lb, &rb).map_err(ch)?;
        let out = match &result {
            ChType::Decimal { precision, scale } => {
                let wide = |s: u8| ChType::Decimal {
                    precision: 38,
                    scale: s,
                };
                let as_decimal = |t: &ChType| match t {
                    ChType::Decimal { scale, .. } => wide(*scale),
                    _ => wide(0),
                };
                if *precision > 38 {
                    return Err(ch(ChError::unsupported(format!("Arithmetic on {result}"))));
                }
                let computed = match op {
                    Arith::Plus | Arith::Minus => {
                        let w = wide(*scale);
                        binary(self.compute_as(l, &w)?, operator, self.compute_as(r, &w)?)
                    }
                    Arith::Multiply => binary(
                        self.compute_as(l, &as_decimal(&lb))?,
                        operator,
                        self.compute_as(r, &as_decimal(&rb))?,
                    ),
                    Arith::Divide => {
                        // ClickHouse truncates to the dividend's scale;
                        // Arrow's quotient has four more digits, and a
                        // cast would round them: drop them first.
                        let q = binary(
                            self.compute_as(l, &as_decimal(&lb))?,
                            Operator::Divide,
                            self.compute_as(r, &as_decimal(&rb))?,
                        );
                        let unit = ScalarValue::Decimal128(Some(1), 38, *scale as i8);
                        binary(
                            q.clone(),
                            Operator::Minus,
                            binary(q, Operator::Modulo, lit(unit)),
                        )
                    }
                    Arith::Modulo => {
                        if !matches!(rb, ChType::Decimal { .. })
                            || !matches!(lb, ChType::Decimal { .. })
                        {
                            return Err(ch(ChError::unsupported(
                                "modulo of a Decimal and an integer (ClickHouse 26.9 reads the integer as the decimal's raw ticks)",
                            )));
                        }
                        let w = wide(*scale);
                        binary(self.compute_as(l, &w)?, operator, self.compute_as(r, &w)?)
                    }
                    Arith::IntDiv => unreachable!("intDiv is a function"),
                };
                let (dt, _) = data_type(&result).map_err(ch)?;
                Expr::Cast(Cast::new(Box::new(computed), dt))
            }
            _ => match op {
                Arith::Modulo if result != ChType::Float64 => {
                    // The remainder of the exact values (ClickHouse computes it
                    // in the operands' own types), then the result's width.
                    let t = rules::comparison(&lb, &rb).map_err(ch)?;
                    let m = binary(self.compute_as(l, &t)?, operator, self.compute_as(r, &t)?);
                    self.compute_as_expr(m, &t, &result)?
                }
                _ => binary(
                    self.compute_as(l, &result)?,
                    operator,
                    self.compute_as(r, &result)?,
                ),
            },
        };
        let t = if nullable_result {
            nullable(result)
        } else {
            result
        };
        tag(out, &t)
    }

    /// `e` (of ClickHouse type `from`) cast to the Arrow type of `to`.
    fn compute_as_expr(&self, e: Expr, from: &ChType, to: &ChType) -> Result<Expr> {
        if from == to {
            return Ok(e);
        }
        let (dt, _) = data_type(&base(to)).map_err(ch)?;
        let (from_dt, _) = data_type(&base(from)).map_err(ch)?;
        Ok(wrap::convert(e, &from_dt, &dt))
    }

    fn date_arithmetic(
        &self,
        l: Expr,
        lb: &ChType,
        op: Arith,
        r: Expr,
        rb: &ChType,
        result: &ChType,
    ) -> Result<Expr> {
        let nullable_result = self.ch_type(&l)?.is_nullable() || self.ch_type(&r)?.is_nullable();
        let wrap = |e: Expr, t: &ChType| {
            tag(
                e,
                &if nullable_result {
                    nullable(t.clone())
                } else {
                    t.clone()
                },
            )
        };
        let i64_of = |e: Expr| Expr::Cast(Cast::new(Box::new(e), DataType::Int64));
        // Ticks of the Arrow layout per ClickHouse unit (a day, a second).
        let per_unit = |t: &ChType| -> i64 {
            match t {
                ChType::DateTime64(p, _) => match loams_house_types::arrow::time_unit(*p) {
                    TimeUnit::Second => 1,
                    TimeUnit::Millisecond => 1_000,
                    TimeUnit::Microsecond => 1_000_000,
                    TimeUnit::Nanosecond => 1_000_000_000,
                },
                _ => 1,
            }
        };
        let time = |t: &ChType| {
            matches!(
                t,
                ChType::Date | ChType::Date32 | ChType::DateTime(_) | ChType::DateTime64(..)
            )
        };
        if time(lb) && time(rb) {
            // `Date - Date`, `DateTime - DateTime`: Int32.
            let diff = binary(i64_of(l), Operator::Minus, i64_of(r));
            return wrap(
                Expr::Cast(Cast::new(Box::new(diff), DataType::Int32)),
                result,
            );
        }
        let (t, n, operator) = if time(lb) {
            (
                l,
                r,
                if op == Arith::Minus {
                    Operator::Minus
                } else {
                    Operator::Plus
                },
            )
        } else {
            (r, l, Operator::Plus)
        };
        let scale = per_unit(result);
        let n = i64_of(n);
        let n = if scale == 1 {
            n
        } else {
            binary(n, Operator::Multiply, lit(ScalarValue::Int64(Some(scale))))
        };
        let ticks = binary(i64_of(t), operator, n);
        let (dt, _) = data_type(result).map_err(ch)?;
        let back = match &dt {
            DataType::Date32 => Expr::Cast(Cast::new(
                Box::new(Expr::Cast(Cast::new(Box::new(ticks), DataType::Int32))),
                DataType::Date32,
            )),
            other => Expr::Cast(Cast::new(Box::new(ticks), other.clone())),
        };
        wrap(back, result)
    }

    /// The least supertype of `exprs`' ClickHouse types, and each of them
    /// converted to it.
    fn to_supertype(&self, exprs: Vec<Expr>, extra: &[ChType]) -> Result<(Vec<Expr>, ChType)> {
        let mut exprs = exprs
            .into_iter()
            .map(|e| self.value(e))
            .collect::<Result<Vec<_>>>()?;
        let mut types = exprs
            .iter()
            .map(|e| self.ch_type(e))
            .collect::<Result<Vec<_>>>()?;
        types.extend_from_slice(extra);
        let t = rules::least_supertype(&types).map_err(ch)?;
        for e in &mut exprs {
            let x = std::mem::replace(e, lit(ScalarValue::Null));
            *e = self.cast_to(x, &t)?;
        }
        Ok((exprs, t))
    }

    fn function(
        &self,
        func: Arc<datafusion::logical_expr::ScalarUDF>,
        args: Vec<Expr>,
    ) -> Result<Expr> {
        let name = func.name().to_string();
        if name == COND {
            let [arg]: [Expr; 1] = args
                .try_into()
                .map_err(|_| ch(ChError::logical_error("__ch_cond takes one argument")))?;
            return self.predicate(arg, Context::Filter);
        }
        let mut args = args
            .into_iter()
            .map(|a| self.settle(a))
            .collect::<Result<Vec<_>>>()?;
        if name == "coalesce" {
            // Arguments after the first that cannot be NULL are never read.
            let mut keep = Vec::new();
            for a in args {
                let stop = !self.ch_type(&a)?.is_nullable();
                keep.push(a);
                if stop {
                    break;
                }
            }
            let any_not_null = keep
                .iter()
                .map(|a| self.ch_type(a).map(|t| !t.is_nullable()))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .any(|b| b);
            let (keep, t) = self.to_supertype(keep, &[])?;
            let _ = (t, any_not_null);
            args = keep;
        } else if let Some(positions) = supertype_arguments(&name, args.len()) {
            let picked: Vec<Expr> = positions.iter().map(|i| args[*i].clone()).collect();
            let (converted, _) = self.to_supertype(picked, &[])?;
            for (i, e) in positions.into_iter().zip(converted) {
                args[i] = e;
            }
        }
        Ok(Expr::ScalarFunction(ScalarFunction::new_udf(func, args)))
    }

    fn case(&self, case: Case) -> Result<Expr> {
        let Case {
            expr,
            when_then_expr,
            else_expr,
        } = case;
        if expr.is_some() {
            // `CASE x WHEN …` (plan HE1 Task 33) is planned as written.
            return Ok(Expr::Case(Case {
                expr,
                when_then_expr,
                else_expr,
            }));
        }
        let mut whens = Vec::with_capacity(when_then_expr.len());
        let mut thens = Vec::with_capacity(when_then_expr.len() + 1);
        for (w, t) in when_then_expr {
            let w = self.value(*w)?;
            let wt = base(&self.ch_type(&w)?);
            if !matches!(wt, ChType::UInt8 | ChType::Bool | ChType::Nothing) {
                return Err(ch(ChError::illegal_type_of_argument(format!(
                    "Illegal type {} of argument (condition) of function multiIf. Must be UInt8",
                    self.ch_type(&w)?
                ))));
            }
            whens.push(self.predicate(w, Context::Function("multiIf"))?);
            thens.push(*t);
        }
        let has_else = else_expr.is_some();
        if let Some(e) = else_expr {
            thens.push(*e);
        }
        let extra = if has_else {
            vec![]
        } else {
            vec![ChType::Nullable(Box::new(ChType::Nothing))]
        };
        let (mut thens, t) = self.to_supertype(thens, &extra)?;
        let else_expr = if has_else { thens.pop() } else { None };
        let out = Expr::Case(Case {
            expr: None,
            when_then_expr: whens
                .into_iter()
                .zip(thens)
                .map(|(w, t)| (Box::new(w), Box::new(t)))
                .collect(),
            else_expr: else_expr.map(Box::new),
        });
        let read = from_arrow(self.field(&out)?.as_ref()).ok();
        if read.as_ref() == Some(&t) || data_type(&t).is_err() {
            Ok(out)
        } else {
            tag(out, &t)
        }
    }

    fn negate(&self, x: Expr) -> Result<Expr> {
        let x = self.value(x)?;
        let t = self.ch_type(&x)?;
        let result = rules::negate(&base(&t)).map_err(ch)?;
        let x = self.compute_as(x, &result)?;
        let t = if t.is_nullable() {
            nullable(result)
        } else {
            result
        };
        tag(Expr::Negative(Box::new(x)), &t)
    }

    fn in_list(&self, list: InList) -> Result<Expr> {
        let InList {
            expr,
            list,
            negated,
        } = list;
        let expr = self.value(*expr)?;
        let et = self.ch_type(&expr)?;
        let mut items = Vec::with_capacity(list.len());
        for item in list {
            let item = self.value(item)?;
            items.push(match self.constant_as(item, &et)? {
                Ok(x) | Err(x) => x,
            });
        }
        let mut t = base(&et);
        for item in &items {
            let it = base(&self.ch_type(item)?);
            if it == ChType::Nothing {
                continue;
            }
            if is_string_type(&t) && is_string_type(&it) {
                continue;
            }
            t = rules::comparison(&t, &it).map_err(ch)?;
        }
        if is_string_type(&t) {
            return Ok(Expr::InList(InList::new(Box::new(expr), items, negated)));
        }
        let expr = self.compute_as(expr, &t)?;
        let items = items
            .into_iter()
            .map(|i| self.compute_as(i, &t))
            .collect::<Result<Vec<_>>>()?;
        Ok(Expr::InList(InList::new(Box::new(expr), items, negated)))
    }
}

struct Rewriter<'a, 'b> {
    types: &'a HouseTypes<'b>,
}

impl TreeNodeRewriter for Rewriter<'_, '_> {
    type Node = Expr;

    fn f_up(&mut self, e: Expr) -> Result<Transformed<Expr>> {
        let t = self.types;
        Ok(Transformed::yes(match e {
            Expr::Literal(v, None) => match literal::narrow(&v) {
                Some(n) => lit(n),
                None => return Ok(Transformed::no(Expr::Literal(v, None))),
            },
            Expr::BinaryExpr(BinaryExpr { left, op, right }) => {
                if let Some(a) = arith_of(op) {
                    t.arithmetic(*left, a, *right)?
                } else if is_comparison(op) {
                    t.compare(*left, op, *right)?
                } else if matches!(op, Operator::And | Operator::Or) {
                    let f = if op == Operator::And { "and" } else { "or" };
                    binary(
                        t.predicate(*left, Context::Function(f))?,
                        op,
                        t.predicate(*right, Context::Function(f))?,
                    )
                } else {
                    binary(t.value(*left)?, op, t.value(*right)?)
                }
            }
            Expr::Not(x) => Expr::Not(Box::new(t.predicate(*x, Context::Function("not"))?)),
            Expr::Negative(x) => t.negate(*x)?,
            Expr::Between(Between {
                expr,
                negated,
                low,
                high,
            }) => {
                // ClickHouse reads `x BETWEEN a AND b` as
                // `x >= a AND x <= b`, each comparison typed on its own.
                let ge = t.compare((*expr).clone(), Operator::GtEq, *low)?;
                let le = t.compare(*expr, Operator::LtEq, *high)?;
                let both = binary(ge, Operator::And, le);
                if negated {
                    Expr::Not(Box::new(both))
                } else {
                    both
                }
            }
            Expr::InList(list) => t.in_list(list)?,
            Expr::Case(case) => t.case(case)?,
            Expr::ScalarFunction(ScalarFunction { func, args }) => t.function(func, args)?,
            Expr::AggregateFunction(AggregateFunction {
                func,
                params:
                    AggregateFunctionParams {
                        args,
                        distinct,
                        filter,
                        order_by,
                        null_treatment,
                    },
            }) => {
                let args = args
                    .into_iter()
                    .map(|a| t.settle(a))
                    .collect::<Result<Vec<_>>>()?;
                let filter = match filter {
                    Some(f) => Some(Box::new(t.predicate(*f, Context::Filter)?)),
                    None => None,
                };
                Expr::AggregateFunction(AggregateFunction {
                    func,
                    params: AggregateFunctionParams {
                        args,
                        distinct,
                        filter,
                        order_by,
                        null_treatment,
                    },
                })
            }
            Expr::WindowFunction(w) => {
                let WindowFunction { fun, params } = *w;
                let WindowFunctionParams { args, .. } = &params;
                let args = args
                    .iter()
                    .cloned()
                    .map(|a| t.settle(a))
                    .collect::<Result<Vec<_>>>()?;
                Expr::from(WindowFunction {
                    fun,
                    params: WindowFunctionParams { args, ..params },
                })
            }
            Expr::Cast(Cast { expr, field }) => Expr::Cast(Cast {
                expr: Box::new(t.value(*expr)?),
                field,
            }),
            other => return Ok(Transformed::no(other)),
        }))
    }
}
