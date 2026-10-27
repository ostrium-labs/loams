//! Kernels where ClickHouse's arithmetic differs from Arrow's (plan HE1 Task
//! 24). The operators themselves (`+`, `-`, `*`, `/`, `%`, comparisons) stay
//! DataFusion `BinaryExpr`s, typed by `loams_house_sql::coerce`; only the
//! functions with no operator are here.

use std::sync::Arc;

use datafusion::logical_expr::ScalarUDF;

pub mod int_div;

/// The type-rule functions, by their ClickHouse names.
pub fn scalars() -> Vec<Arc<ScalarUDF>> {
    vec![
        Arc::new(ScalarUDF::new_from_impl(int_div::IntDiv::new(false))),
        Arc::new(ScalarUDF::new_from_impl(int_div::IntDiv::new(true))),
    ]
}
