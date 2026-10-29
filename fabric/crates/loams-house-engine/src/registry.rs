//! The functions the engine registers with DataFusion, by their ClickHouse
//! names (plan HE1 Tasks 28–33 fill it).
//!
//! Aggregates are `loams_house_exec::agg` (Task 28).

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
    loams_house_exec::agg::register(&mut r);
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
    // Date and time functions (plan HE1 Task 30).
    loams_house_exec::func::datetime::register(&mut r);
    // Strings, regular expressions, hashes and UUIDs (plan HE1 Task 31).
    loams_house_exec::func::strings::register(&mut r);
    loams_house_exec::func::hash::register(&mut r);
    crate::system::functions::register(&mut r);
    // Conversions, conditionals, maths and JSON (plan HE1 Task 33).
    loams_house_exec::func::register_t33(&mut r);
    r
}
