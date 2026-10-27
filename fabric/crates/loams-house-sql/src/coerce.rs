//! ClickHouse coercion replaces DataFusion's TypeCoercion analyzer (HE1 Task 24).
//! Operators are type-aware kernels; conditionals lower to lazy physical CASE.
use datafusion::{
    arrow::datatypes::DataType,
    common::{
        DFSchema, Result, ScalarValue,
        config::ConfigOptions,
        tree_node::{Transformed, TreeNode},
    },
    logical_expr::{
        Expr, ExprSchemable, LogicalPlan,
        expr::{Case, ScalarFunction},
        utils::merge_schema,
    },
    optimizer::analyzer::AnalyzerRule,
    sql::sqlparser::ast::{self, VisitMut, VisitorMut},
};
use loams_house_exec::ops::{self, Op};
use loams_house_types::{ChError, ChType};
use std::{ops::ControlFlow, sync::Arc};

fn call(name: &str, args: Vec<ast::Expr>) -> ast::Expr {
    ast::Expr::Function(ast::Function {
        name: ast::ObjectName::from(vec![ast::Ident::with_quote('"', name)]),
        uses_odbc_syntax: false,
        parameters: ast::FunctionArguments::None,
        args: ast::FunctionArguments::List(ast::FunctionArgumentList {
            duplicate_treatment: None,
            args: args
                .into_iter()
                .map(|a| ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(a)))
                .collect(),
            clauses: vec![],
        }),
        filter: None,
        null_treatment: None,
        over: None,
        within_group: vec![],
    })
}
fn boolean(e: ast::Expr) -> ast::Expr {
    ast::Expr::Cast {
        kind: ast::CastKind::Cast,
        expr: Box::new(e),
        data_type: ast::DataType::Bool,
        array: false,
        format: None,
    }
}
struct Lower;
impl VisitorMut for Lower {
    type Break = ChError;
    fn post_visit_expr(&mut self, e: &mut ast::Expr) -> ControlFlow<ChError> {
        use ast::Expr as E;
        *e = match e.clone() {
            E::BinaryOp { left, op, right } => {
                match crate::names::operator_function(&op).filter(|n| Op::named(n).is_some()) {
                    Some(name) => call(name, vec![*left, *right]),
                    None => return ControlFlow::Continue(()),
                }
            }
            E::UnaryOp {
                op: ast::UnaryOperator::Not,
                expr,
            } => call("not", vec![*expr]),
            E::UnaryOp {
                op: ast::UnaryOperator::Minus,
                expr,
            } => call("negate", vec![*expr]),
            E::Cast {
                kind: ast::CastKind::Cast,
                expr,
                data_type,
                ..
            } => call(
                "__ch_cast",
                vec![
                    *expr,
                    E::Value(
                        ast::Value::SingleQuotedString(crate::context::type_text(&data_type))
                            .into(),
                    ),
                ],
            ),
            E::IsNull(x) => call("isNull", vec![*x]),
            E::IsNotNull(x) => call("isNotNull", vec![*x]),
            E::Case {
                operand,
                conditions,
                else_result,
                ..
            } => {
                let mut args = Vec::new();
                for when in conditions {
                    args.push(match &operand {
                        Some(operand) => call("equals", vec![*operand.clone(), when.condition]),
                        None => when.condition,
                    });
                    args.push(when.result);
                }
                args.push(
                    else_result
                        .map(|e| *e)
                        .unwrap_or_else(|| E::Value(ast::Value::Null.into())),
                );
                call("multiIf", args)
            }
            E::Array(array) => call("array", array.elem),
            E::Between {
                expr,
                negated,
                low,
                high,
            } => {
                let value = call(
                    "and",
                    vec![
                        call("greaterOrEquals", vec![*expr.clone(), *low]),
                        call("lessOrEquals", vec![*expr, *high]),
                    ],
                );
                if negated {
                    call("not", vec![value])
                } else {
                    value
                }
            }
            E::InList {
                expr,
                list,
                negated,
            } => {
                let values = list
                    .into_iter()
                    .map(|v| call("equals", vec![*expr.clone(), v]))
                    .collect::<Vec<_>>();
                let value = if values.is_empty() {
                    E::Value(ast::Value::Boolean(false).into())
                } else {
                    call("or", values)
                };
                if negated {
                    call("not", vec![value])
                } else {
                    value
                }
            }
            _ => return ControlFlow::Continue(()),
        };
        ControlFlow::Continue(())
    }
    fn post_visit_select(&mut self, s: &mut ast::Select) -> ControlFlow<ChError> {
        for condition in [&mut s.selection, &mut s.having] {
            if let Some(e) = condition.take() {
                *condition = Some(boolean(e));
            }
        }
        for table in &mut s.from {
            for join in &mut table.joins {
                use ast::JoinOperator as J;
                let constraint = match &mut join.join_operator {
                    J::Join(c)
                    | J::Inner(c)
                    | J::Left(c)
                    | J::LeftOuter(c)
                    | J::Right(c)
                    | J::RightOuter(c)
                    | J::FullOuter(c)
                    | J::CrossJoin(c) => Some(c),
                    _ => None,
                };
                if let Some(ast::JoinConstraint::On(e)) = constraint {
                    *e = boolean(e.clone());
                }
            }
        }
        ControlFlow::Continue(())
    }
}
/// Lower guarded operators only after output names have been taken from SQL text.
pub fn lower(statement: &mut ast::Statement) -> std::result::Result<(), ChError> {
    match statement.visit(&mut Lower) {
        ControlFlow::Continue(()) => Ok(()),
        ControlFlow::Break(e) => Err(e),
    }
}
/// Replaces DataFusion's numeric coercion; its operators never see House numbers.
#[derive(Debug, Default)]
pub struct ClickHouseCoercion;
fn cast(expr: Expr, t: &ChType) -> Expr {
    ops::cast_udf().call(vec![
        expr,
        Expr::Literal(ScalarValue::Utf8(Some(t.to_compact_string())), None),
    ])
}
fn truth(expr: Expr, schema: &DFSchema) -> Result<Expr> {
    expr.cast_to(&DataType::Boolean, schema)
}
fn rewrite(expr: Expr, schema: &DFSchema) -> Result<Transformed<Expr>> {
    let Expr::ScalarFunction(ScalarFunction { func, args }) = &expr else {
        return Ok(Transformed::no(expr));
    };
    let Some(op) = Op::named(func.name()).filter(|o| o.conditional()) else {
        return Ok(Transformed::no(expr));
    };
    let field = expr.to_field(schema)?.1;
    let result = loams_house_types::arrow::from_arrow(&field).map_err(crate::context::external)?;
    let mut pairs = Vec::new();
    let last = match op {
        Op::If => {
            pairs.push((
                Box::new(truth(args[0].clone(), schema)?),
                Box::new(cast(args[1].clone(), &result)),
            ));
            args[2].clone()
        }
        Op::MultiIf => {
            for pair in args[..args.len() - 1].chunks_exact(2) {
                pairs.push((
                    Box::new(truth(pair[0].clone(), schema)?),
                    Box::new(cast(pair[1].clone(), &result)),
                ));
            }
            args.last().expect("else").clone()
        }
        Op::Coalesce | Op::IfNull => {
            // Stop after a guaranteed non-null branch: later types are irrelevant in CH.
            let mut fallback = Expr::Literal(ScalarValue::Null, None);
            for arg in args {
                if !arg.nullable(schema)? {
                    fallback = arg.clone();
                    break;
                }
                pairs.push((
                    Box::new(Expr::IsNotNull(Box::new(arg.clone()))),
                    Box::new(cast(arg.clone(), &result)),
                ));
            }
            fallback
        }
        _ => unreachable!(),
    };
    let name = expr.schema_name().to_string();
    let case = Expr::Case(Case::new(None, pairs, Some(Box::new(cast(last, &result)))));
    Ok(Transformed::yes(case.alias_with_metadata(
        name,
        Some(field.metadata().clone().into()),
    )))
}
impl AnalyzerRule for ClickHouseCoercion {
    fn name(&self) -> &str {
        "clickhouse_type_coercion"
    }
    fn analyze(&self, plan: LogicalPlan, _: &ConfigOptions) -> Result<LogicalPlan> {
        Ok(plan
            .transform_up_with_subqueries(|plan| {
                let schema = merge_schema(&plan.inputs());
                plan.map_expressions(|expr| expr.transform_up(|expr| rewrite(expr, &schema)))?
                    .map_data(|plan| plan.recompute_schema())
            })?
            .data)
    }
}
/// The analyzer list replaces the default TypeCoercion instead of appending to it.
pub fn analyzer_rules() -> Vec<Arc<dyn AnalyzerRule + Send + Sync>> {
    vec![Arc::new(ClickHouseCoercion)]
}
