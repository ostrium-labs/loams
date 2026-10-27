//! What `SqlToRel` needs to plan ClickHouse's boolean contexts at all.
//!
//! DataFusion checks some types while it plans, before any analyzer rule
//! runs: a `Filter` must be boolean, `AND`/`OR` take booleans, and a boolean
//! is no operand of `+` or `=` against a number. ClickHouse takes a number in
//! each of these places. Two small shims keep such statements plannable, and
//! the analyzer rule ([`super::ClickHouseCoercion`]) then gives them
//! ClickHouse's types:
//!
//! * [`mark_conditions`] wraps a `WHERE` or `HAVING` condition that is not
//!   syntactically boolean in [`COND`]`(…)`, which the analyzer rule replaces
//!   with the condition (a number `x` becomes `x != 0`);
//! * [`ClickHouseOperators`], an `ExprPlanner`, turns a number operand of
//!   `AND`/`OR` into `x != 0` and a boolean operand of arithmetic or of a
//!   comparison with a number into `UInt8`.

use std::ops::ControlFlow;
use std::sync::{Arc, OnceLock};

use datafusion::arrow::datatypes::DataType;
use datafusion::common::{DFSchema, Result, ScalarValue};
use datafusion::logical_expr::planner::{ExprPlanner, PlannerResult, RawBinaryExpr};
use datafusion::logical_expr::{
    BinaryExpr, ColumnarValue, Expr, ExprSchemable, Operator, ScalarFunctionArgs, ScalarUDF,
    ScalarUDFImpl, Signature, Volatility,
};
use datafusion::sql::sqlparser::ast::{
    self, BinaryOperator, Expr as SqlExpr, Ident, ObjectName, Query, SetExpr, UnaryOperator,
    VisitMut, VisitorMut,
};
use loams_house_types::ChError;

use crate::context::external;

/// The marker function a non-boolean `WHERE`/`HAVING` condition is wrapped
/// in until the analyzer rule replaces it.
pub const COND: &str = "__ch_cond";

/// Wraps every `WHERE` and `HAVING` condition of `statement` that is not
/// syntactically boolean in [`COND`]`(…)`.
pub fn mark_conditions(statement: &mut ast::Statement) {
    struct Marker;
    impl VisitorMut for Marker {
        type Break = ();
        fn pre_visit_query(&mut self, query: &mut Query) -> ControlFlow<()> {
            mark_set_expr(&mut query.body);
            ControlFlow::Continue(())
        }
    }
    let _ = statement.visit(&mut Marker);
}

fn mark_set_expr(body: &mut SetExpr) {
    match body {
        SetExpr::Select(select) => {
            for c in [&mut select.selection, &mut select.having]
                .into_iter()
                .flatten()
            {
                if !syntactically_boolean(c) {
                    let inner = std::mem::replace(c, SqlExpr::Value(ast::Value::Null.into()));
                    *c = SqlExpr::Function(ast::Function {
                        name: ObjectName::from(vec![Ident::with_quote('"', COND)]),
                        uses_odbc_syntax: false,
                        parameters: ast::FunctionArguments::None,
                        args: ast::FunctionArguments::List(ast::FunctionArgumentList {
                            duplicate_treatment: None,
                            args: vec![ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(
                                inner,
                            ))],
                            clauses: vec![],
                        }),
                        filter: None,
                        null_treatment: None,
                        over: None,
                        within_group: vec![],
                    });
                }
            }
        }
        SetExpr::SetOperation { left, right, .. } => {
            mark_set_expr(left);
            mark_set_expr(right);
        }
        // A nested query is visited as a query of its own.
        _ => {}
    }
}

/// Whether DataFusion types `e` as a boolean whatever its operands are.
fn syntactically_boolean(e: &SqlExpr) -> bool {
    match e {
        SqlExpr::Nested(x) => syntactically_boolean(x),
        SqlExpr::BinaryOp { op, .. } => matches!(
            op,
            BinaryOperator::Eq
                | BinaryOperator::NotEq
                | BinaryOperator::Lt
                | BinaryOperator::LtEq
                | BinaryOperator::Gt
                | BinaryOperator::GtEq
                | BinaryOperator::And
                | BinaryOperator::Or
        ),
        SqlExpr::UnaryOp {
            op: UnaryOperator::Not,
            ..
        } => true,
        SqlExpr::IsNull(_)
        | SqlExpr::IsNotNull(_)
        | SqlExpr::InList { .. }
        | SqlExpr::InSubquery { .. }
        | SqlExpr::Between { .. }
        | SqlExpr::Like { .. }
        | SqlExpr::ILike { .. }
        | SqlExpr::Exists { .. } => true,
        SqlExpr::Value(v) => matches!(v.value, ast::Value::Boolean(_) | ast::Value::Null),
        _ => false,
    }
}

/// The [`COND`] function: planned, never run (the analyzer rule replaces it).
pub fn cond_udf() -> Arc<ScalarUDF> {
    static UDF: OnceLock<Arc<ScalarUDF>> = OnceLock::new();
    UDF.get_or_init(|| Arc::new(ScalarUDF::new_from_impl(Cond::default())))
        .clone()
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct Cond {
    signature: Signature,
}

impl Default for Cond {
    fn default() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl ScalarUDFImpl for Cond {
    fn name(&self) -> &str {
        COND
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }
    fn invoke_with_args(&self, _args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        Err(external(ChError::logical_error(
            "a WHERE condition reached execution without the ClickHouse type rules",
        )))
    }
}

/// Makes ClickHouse's mixes of numbers and booleans plannable (see the
/// module documentation); the analyzer rule types the result.
#[derive(Debug, Default)]
pub struct ClickHouseOperators;

fn is_numeric(dt: &DataType) -> bool {
    match dt {
        DataType::Dictionary(_, v) => is_numeric(v),
        other => other.is_numeric(),
    }
}

/// `x != 0` for a number `x`; `43` for any other non-boolean type.
pub(crate) fn nonzero(x: Expr, dt: &DataType, function: &str) -> Result<Expr> {
    let value_type = match dt {
        DataType::Dictionary(_, v) => v.as_ref().clone(),
        other => other.clone(),
    };
    if !is_numeric(&value_type) {
        return Err(external(ChError::illegal_type_of_argument(format!(
            "Illegal type ({}) of argument of function {function}",
            crate::coerce::ch_text(dt)
        ))));
    }
    let x = if matches!(dt, DataType::Dictionary(..)) {
        x.cast_to(&value_type, &DFSchema::empty())?
    } else {
        x
    };
    let zero = ScalarValue::new_zero(&value_type)?;
    Ok(Expr::BinaryExpr(BinaryExpr::new(
        Box::new(x),
        Operator::NotEq,
        Box::new(Expr::Literal(zero, None)),
    )))
}

impl ExprPlanner for ClickHouseOperators {
    fn plan_binary_op(
        &self,
        mut expr: RawBinaryExpr,
        schema: &DFSchema,
    ) -> Result<PlannerResult<RawBinaryExpr>> {
        let (Ok(lt), Ok(rt)) = (expr.left.get_type(schema), expr.right.get_type(schema)) else {
            return Ok(PlannerResult::Original(expr));
        };
        let boolean = |t: &DataType| matches!(t, DataType::Boolean | DataType::Null);
        match expr.op {
            BinaryOperator::And | BinaryOperator::Or => {
                let function = if expr.op == BinaryOperator::And {
                    "and"
                } else {
                    "or"
                };
                if !boolean(&lt) {
                    expr.left = nonzero(expr.left, &lt, function)?;
                }
                if !boolean(&rt) {
                    expr.right = nonzero(expr.right, &rt, function)?;
                }
            }
            BinaryOperator::Plus
            | BinaryOperator::Minus
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Modulo
            | BinaryOperator::Eq
            | BinaryOperator::NotEq
            | BinaryOperator::Lt
            | BinaryOperator::LtEq
            | BinaryOperator::Gt
            | BinaryOperator::GtEq => {
                let arithmetic = !matches!(
                    expr.op,
                    BinaryOperator::Eq
                        | BinaryOperator::NotEq
                        | BinaryOperator::Lt
                        | BinaryOperator::LtEq
                        | BinaryOperator::Gt
                        | BinaryOperator::GtEq
                );
                // A boolean against a boolean compares as is.
                let mixed = lt == DataType::Boolean && rt != DataType::Boolean
                    || rt == DataType::Boolean && lt != DataType::Boolean;
                if arithmetic || mixed {
                    if lt == DataType::Boolean {
                        expr.left = expr.left.cast_to(&DataType::UInt8, schema)?;
                    }
                    if rt == DataType::Boolean {
                        expr.right = expr.right.cast_to(&DataType::UInt8, schema)?;
                    }
                }
            }
            _ => {}
        }
        Ok(PlannerResult::Original(expr))
    }
}
