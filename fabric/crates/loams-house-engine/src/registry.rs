//! The functions the engine registers with DataFusion, by their ClickHouse
//! names (plan HE1 Tasks 28–33 fill it).
//!
//! **Stand-ins until Task 28.** `count`, `sum`, `min`, `max` and `avg` are
//! DataFusion's own aggregates under ClickHouse's names: the right rows, but
//! not ClickHouse's result types (`count` is `Int64`, not `UInt64`). The
//! oracle reports each difference, and the triage marks it a `bug` of Task 28
//! (D1804); nothing is hidden.

use std::sync::Arc;

use datafusion::arrow::datatypes::DataType;
use datafusion::common::ScalarValue;
use datafusion::logical_expr::{ColumnarValue, Volatility, create_udf};
use loams_house_sql::Registry;

/// The version the House reports (`HouseConfig::version`, HS1 R1.1).
pub const VERSION: &str = "26.9.2.1";

/// The engine's functions.
pub fn registry() -> Registry {
    let mut r = Registry::default();
    use datafusion::functions_aggregate as agg;
    r.aggregates
        .insert("count".into(), agg::count::count_udaf());
    r.aggregates.insert("sum".into(), agg::sum::sum_udaf());
    r.aggregates.insert("min".into(), agg::min_max::min_udaf());
    r.aggregates.insert("max".into(), agg::min_max::max_udaf());
    r.aggregates.insert("avg".into(), agg::average::avg_udaf());
    let version = create_udf(
        "version",
        vec![],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(|_args: &[ColumnarValue]| {
            Ok(ColumnarValue::Scalar(ScalarValue::Utf8(Some(
                VERSION.to_string(),
            ))))
        }),
    );
    r.scalars.insert("version".into(), Arc::new(version));
    loams_house_exec::ops::scalars()
        .into_iter()
        .for_each(|f| _ = r.scalars.insert(f.name().into(), f));
    // Arrays, maps, tuples and lambdas (plan HE1 Task 32).
    loams_house_exec::func::arrays::register(&mut r);
    loams_house_exec::func::maps::register(&mut r);
    loams_house_exec::func::tuples::register(&mut r);
    loams_house_exec::func::lambda::register(&mut r);
    r
}
