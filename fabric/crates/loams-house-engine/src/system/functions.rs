//! `currentDatabase()`, `timezone()` and `serverTimezone()` (plan HE1 Task
//! 38): constants of the statement, registered for each statement with its
//! current database and its session's zone ([`for_statement`]).

use std::sync::Arc;

use datafusion::arrow::datatypes::DataType;
use datafusion::common::ScalarValue;
use datafusion::logical_expr::{ColumnarValue, Volatility, create_udf};
use loams_house_sql::Registry;

use crate::settings::SERVER_TIMEZONE;

fn constant(name: &str, value: &str) -> datafusion::logical_expr::ScalarUDF {
    let value = value.to_string();
    create_udf(
        name,
        vec![],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(move |_args: &[ColumnarValue]| {
            Ok(ColumnarValue::Scalar(ScalarValue::Utf8(Some(
                value.clone(),
            ))))
        }),
    )
}

fn set(r: &mut Registry, database: &str, timezone: &str) {
    r.scalars.insert(
        "currentDatabase".into(),
        Arc::new(constant("currentDatabase", database)),
    );
    r.scalars
        .insert("timezone".into(), Arc::new(constant("timezone", timezone)));
    r.scalars.insert(
        "serverTimezone".into(),
        Arc::new(constant("serverTimezone", SERVER_TIMEZONE)),
    );
}

/// Registers the functions at the dev server's defaults (`default`, the
/// server's zone), so `system.functions` lists them.
pub fn register(r: &mut Registry) {
    set(r, "default", SERVER_TIMEZONE);
}

/// `base` with the functions answering for a statement in `database` whose
/// session zone is `timezone`.
pub fn for_statement(base: &Registry, database: &str, timezone: &str) -> Registry {
    let mut r = base.clone();
    set(&mut r, database, timezone);
    r
}
