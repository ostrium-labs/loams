//! `untuple(t)` as a select item (plan HE1 Task 32): one output column per
//! element of `t`, as ClickHouse's analyzer expands it
//! (`QueryAnalyzer::resolveFunction`, `src/Analyzer/Resolve/QueryAnalyzer.cpp`,
//! Apache-2.0). Element `n` of the tuple is `tupleElement(t, 'n')` (the
//! element's name; an unnamed tuple's are `1`, `2`, …), named
//! `tupleElement(<t>, '<n>')`, or `<alias>.<n>` when the item has an alias.
//!
//! [`crate::scope`] expands `untuple` of a column or a tuple literal before
//! planning (plan HE1 Task 25) and leaves `untuple` of any other expression
//! to this pass, which runs on the logical plan after `SqlToRel`, where the
//! tuple's type is known. An `untuple` anywhere else stays and is refused
//! when it runs.

use std::sync::Arc;

use datafusion::arrow::datatypes::DataType;
use datafusion::common::tree_node::{Transformed, TreeNode};
use datafusion::common::{Result, ScalarValue};
use datafusion::logical_expr::expr::{Alias, ScalarFunction};
use datafusion::logical_expr::{Expr, ExprSchemable, LogicalPlan, Projection, ScalarUDF};
use loams_house_types::ty::quote;

use crate::context::Registry;

/// The plan with every select item `untuple(t)` expanded.
pub fn expand(plan: LogicalPlan, registry: &Registry) -> Result<LogicalPlan> {
    let Some(tuple_element) = registry.scalars.get("tupleElement").cloned() else {
        return Ok(plan);
    };
    plan.transform_up(|node| match node {
        LogicalPlan::Projection(p) if p.expr.iter().any(is_untuple_item) => {
            let mut exprs = Vec::with_capacity(p.expr.len());
            for e in &p.expr {
                if is_untuple_item(e) {
                    exprs.extend(elements(e, &p, &tuple_element)?);
                } else {
                    exprs.push(e.clone());
                }
            }
            let projection = Projection::try_new(exprs, Arc::clone(&p.input))?;
            Ok(Transformed::yes(LogicalPlan::Projection(projection)))
        }
        other => Ok(Transformed::no(other)),
    })
    .map(|t| t.data)
}

fn untuple_call(e: &Expr) -> Option<&ScalarFunction> {
    match e {
        Expr::ScalarFunction(f) if f.func.name() == "untuple" => Some(f),
        _ => None,
    }
}

fn is_untuple_item(e: &Expr) -> bool {
    match e {
        Expr::Alias(Alias { expr, .. }) => untuple_call(expr).is_some(),
        other => untuple_call(other).is_some(),
    }
}

/// The element columns of the select item `e`, an (aliased) `untuple(t)`.
fn elements(e: &Expr, p: &Projection, tuple_element: &Arc<ScalarUDF>) -> Result<Vec<Expr>> {
    let (call, alias) = match e {
        Expr::Alias(Alias { expr, name, .. }) => (expr.as_ref(), Some(name.as_str())),
        other => (other, None),
    };
    let Some(f) = untuple_call(call) else {
        return Ok(vec![e.clone()]);
    };
    let Some(arg) = f.args.first() else {
        return Ok(vec![e.clone()]);
    };
    let DataType::Struct(fields) = arg.get_type(p.input.schema().as_ref())? else {
        // Not a tuple: left for the function to refuse.
        return Ok(vec![e.clone()]);
    };
    // The guard names an unaliased item by its text, `untuple(<t>)`; a
    // client's own alias names the elements `<alias>.<n>`.
    let text = alias.and_then(|a| a.strip_prefix("untuple(")?.strip_suffix(')'));
    let generated = text.is_some() || alias.is_none();
    let arg_text = text.map(str::to_string).unwrap_or_else(|| arg.to_string());
    Ok(fields
        .iter()
        .map(|field| {
            let element = field.name().clone();
            let name = if generated {
                format!("tupleElement({arg_text}, {})", quote(&element))
            } else {
                format!("{}.{element}", alias.unwrap_or_default())
            };
            Expr::ScalarFunction(ScalarFunction::new_udf(
                Arc::clone(tuple_element),
                vec![
                    arg.clone(),
                    Expr::Literal(ScalarValue::Utf8(Some(element)), None),
                ],
            ))
            .alias(name)
        })
        .collect())
}
