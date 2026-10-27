//! `lower`, `upper` (ASCII letters only, as ClickHouse) and `lowerUTF8`,
//! `upperUTF8` (Unicode's full case mappings: `upperUTF8('ß')` is `SS`,
//! `lowerUTF8('İ')` is `i̇`, as measured).

use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, StringBuilder};
use loams_house_sql::Registry;
use loams_house_types::{ChError, ChType};

use crate::func::chudf::{Call, ChFunction, check_arity, want_string};

/// A string-to-string kernel applying `f` to each value.
pub(crate) fn map_strings(
    call: &Call<'_>,
    i: usize,
    f: impl Fn(&str) -> Result<String, ChError>,
) -> Result<ArrayRef, ChError> {
    let s = call.strs(i)?;
    let mut out = StringBuilder::with_capacity(call.rows, 0);
    for row in 0..call.rows {
        match s.get(row) {
            Some(v) => out.append_value(f(v)?),
            None => out.append_null(),
        }
    }
    Ok(Arc::new(out.finish()))
}

fn one_string(name: &'static str, f: fn(&str) -> String) -> ChFunction {
    ChFunction::new(
        name,
        Arc::new(move |args, _| {
            check_arity(name, args.len(), 1, 1)?;
            want_string(name, args, 0)?;
            Ok(ChType::String)
        }),
        Arc::new(move |call| map_strings(call, 0, |s| Ok(f(s)))),
    )
}

pub(super) fn register(r: &mut Registry) {
    one_string("lower", str::to_ascii_lowercase).register(r);
    one_string("upper", str::to_ascii_uppercase).register(r);
    one_string("lowerUTF8", str::to_lowercase).register(r);
    one_string("upperUTF8", str::to_uppercase).register(r);
}
