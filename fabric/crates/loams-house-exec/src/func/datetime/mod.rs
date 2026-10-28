//! ClickHouse's date and time functions (plan HE1 Task 30).
//!
//! Each function is a [`udf::ChFunction`]: a DataFusion `ScalarUDF` typed
//! from its arguments' ClickHouse types, as ClickHouse types it, with the
//! result's type in the field's `loams.ch.type`. Time zones come from the
//! function's zone argument, else the value type's own zone, else the
//! session's ([`zone`]).
//!
//! * [`convert`]: `toDate`, `toDate32`, `toDateTime`, `toDateTime64`,
//!   `toTimeZone`, `toUnixTimestamp[64*]`, `fromUnixTimestamp64*`, `now`,
//!   `now64`, `parseDateTime[64]BestEffort[US][OrNull|OrZero]`;
//! * [`calendar`]: `toYear` … `toSecond`, `toYYYYMM[DD[hhmmss]]`,
//!   `toDayOfWeek`, `toDayOfYear`, the `toStartOf*` family,
//!   `toStartOfInterval`, `dateTrunc`;
//! * [`interval`]: `toInterval*`, `add*`/`subtract*`, `dateDiff`, `timeDiff`,
//!   and [`interval::DatePlanner`], which plans `plus` and `minus` on dates,
//!   times and intervals (`INTERVAL n UNIT` literals are rewritten to
//!   `toInterval*` calls by the front end, `loams_house_sql::temporal`);
//! * [`format`]: `formatDateTime`.

pub mod calendar;
pub mod convert;
pub mod format;
pub mod interval;
pub mod parse;
pub mod udf;
pub mod value;
pub mod zone;

use std::sync::Arc;

use loams_house_sql::Registry;
use loams_house_sql::functions::internal_name;

/// Every function this module registers, by ClickHouse name.
pub fn names() -> Vec<String> {
    let (iv, minus) = interval::functions();
    convert::functions()
        .into_iter()
        .chain(calendar::functions())
        .chain(iv)
        .chain(format::functions())
        .chain(std::iter::once(minus))
        .map(|f| datafusion::logical_expr::ScalarUDFImpl::name(&f).to_string())
        .collect()
}

/// Registers the date and time functions and the date arithmetic planner.
pub fn register(r: &mut Registry) {
    let (iv, minus) = interval::functions();
    for f in convert::functions()
        .into_iter()
        .chain(calendar::functions())
        .chain(iv)
        .chain(format::functions())
        .chain(std::iter::once(minus))
    {
        let name = internal_name(datafusion::logical_expr::ScalarUDFImpl::name(&f));
        r.scalars.insert(name, udf::udf(f));
    }
    if let Some(p) = interval::planner(&r.scalars) {
        r.expr_planners.push(Arc::new(p));
    }
}
