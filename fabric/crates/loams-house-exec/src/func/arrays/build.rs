//! Functions that build arrays: `array` (and `[…]`), `range`, `arrayConcat`,
//! `arrayZip`, and `arrayJoin`'s typing (ClickHouse `src/Functions/array/
//! array.cpp`, `range.cpp`, `arrayConcat.cpp`, `arrayZip.cpp`, Apache-2.0).

use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, Int64Array, StructArray, UInt64Array, new_empty_array,
};
use datafusion::arrow::compute::{concat, interleave};
use datafusion::arrow::datatypes::DataType;
use loams_house_sql::Registry;
use loams_house_types::arrow::data_type;
use loams_house_types::{ChError, ChType};

use crate::func::nested::supertype::supertype;
use crate::func::nested::values::{
    ListParts, build_list, gather_list, ints, item_field, take_indexes, to_type,
};
use crate::func::nested::{
    Composite, arrow_err, check_arity, element_of, illegal_type, sizes_dont_match,
};

/// The most elements `range` builds in one call (ClickHouse's
/// `function_range_max_elements_in_block` default).
pub const RANGE_MAX_ELEMENTS: usize = 500_000_000;

/// `69 ARGUMENT_OUT_OF_BOUND`.
fn out_of_bound(message: String) -> ChError {
    ChError::argument_out_of_bound(message)
}

/// Registers `array`, `range`, `arrayConcat`, `arrayZip` and `arrayJoin`.
pub fn register(r: &mut Registry) {
    // `array(x, …)`: `[x, …]`, of the elements' least supertype.
    Composite::new(
        "array",
        Arc::new(|types, _| Ok(ChType::Array(Box::new(supertype(types)?)))),
        Arc::new(|c| {
            let ChType::Array(elem) = c.ret else {
                return Err(ChError::logical_error("array is not typed Array"));
            };
            let list_type = data_type(c.ret)?.0;
            if c.args.is_empty() {
                let values = new_empty_array(item_field(&list_type)?.data_type());
                return build_list(&list_type, std::iter::repeat_n(0, c.rows), values);
            }
            let columns = c
                .args
                .iter()
                .map(|a| to_type(a, elem))
                .collect::<Result<Vec<_>, _>>()?;
            let refs: Vec<&dyn Array> = columns.iter().map(|a| a.as_ref()).collect();
            let order: Vec<(usize, usize)> = (0..c.rows)
                .flat_map(|r| (0..columns.len()).map(move |i| (i, r)))
                .collect();
            let values = interleave(&refs, &order).map_err(arrow_err)?;
            build_list(
                &list_type,
                std::iter::repeat_n(columns.len(), c.rows),
                values,
            )
        }),
    )
    .own_nulls()
    .register(r);

    // `range([start, ] end [, step])`: integers from `start` (0) up to, not
    // including, `end`, by `step` (1).
    Composite::new(
        "range",
        Arc::new(|types, _| {
            check_arity("range", types.len(), 1, 3)?;
            for (i, t) in types.iter().enumerate() {
                if !is_integer(t) {
                    return Err(illegal_type("range", i, t));
                }
            }
            Ok(ChType::Array(Box::new(supertype(types)?)))
        }),
        Arc::new(|c| {
            let ChType::Array(elem) = c.ret else {
                return Err(ChError::logical_error("range is not typed Array"));
            };
            let cols = c
                .args
                .iter()
                .map(ints)
                .collect::<Result<Vec<_>, _>>()?;
            let mut lengths = Vec::with_capacity(c.rows);
            let mut values: Vec<i128> = Vec::new();
            for r in 0..c.rows {
                let (start, end, step) = match cols.len() {
                    1 => (Some(0), cols[0][r], Some(1)),
                    2 => (cols[0][r], cols[1][r], Some(1)),
                    _ => (cols[0][r], cols[1][r], cols[2][r]),
                };
                let (Some(start), Some(end), Some(step)) = (start, end, step) else {
                    lengths.push(0);
                    continue;
                };
                if step == 0 {
                    return Err(out_of_bound(
                        "A call to function range with step 0 would produce an infinite array"
                            .to_string(),
                    ));
                }
                let count = if step > 0 && end > start {
                    (end - start + step - 1) / step
                } else if step < 0 && start > end {
                    (start - end + (-step) - 1) / (-step)
                } else {
                    0
                };
                let count = usize::try_from(count).unwrap_or(usize::MAX);
                if values.len().saturating_add(count) > RANGE_MAX_ELEMENTS {
                    return Err(out_of_bound(format!(
                        "A call to function range would produce {} elements, but at most {RANGE_MAX_ELEMENTS} are allowed",
                        values.len().saturating_add(count)
                    )));
                }
                lengths.push(count);
                let mut v = start;
                for _ in 0..count {
                    values.push(v);
                    v += step;
                }
            }
            let list_type = data_type(c.ret)?.0;
            let values: ArrayRef = if matches!(**elem, ChType::UInt64) {
                Arc::new(UInt64Array::from(
                    values.iter().map(|v| *v as u64).collect::<Vec<_>>(),
                ))
            } else {
                Arc::new(Int64Array::from(
                    values.iter().map(|v| *v as i64).collect::<Vec<_>>(),
                ))
            };
            build_list(&list_type, lengths, values)
        }),
    )
    .register(r);

    // `arrayConcat(a, b, …)`: the arrays end to end, of their elements'
    // least supertype.
    Composite::new(
        "arrayConcat",
        Arc::new(|types, _| {
            check_arity("arrayConcat", types.len(), 1, usize::MAX)?;
            let mut elems = Vec::with_capacity(types.len());
            for i in 0..types.len() {
                elems.push(element_of("arrayConcat", types, i)?.clone());
            }
            Ok(ChType::Array(Box::new(supertype(&elems)?)))
        }),
        Arc::new(|c| {
            let list_type = data_type(c.ret)?.0;
            let parts = c
                .args
                .iter()
                .map(|a| ListParts::of(&to_type(a, c.ret)?))
                .collect::<Result<Vec<_>, _>>()?;
            let mut bases = Vec::with_capacity(parts.len());
            let mut base = 0u64;
            for p in &parts {
                bases.push(base);
                base += p.values.len() as u64;
            }
            let all: Vec<&dyn Array> = parts.iter().map(|p| p.values.as_ref()).collect();
            let values = concat(&all).map_err(arrow_err)?;
            let picks: Vec<Vec<u64>> = (0..c.rows)
                .map(|r| {
                    parts
                        .iter()
                        .zip(&bases)
                        .flat_map(|(p, b)| p.range(r).map(move |j| b + j as u64))
                        .collect()
                })
                .collect();
            gather_list(&list_type, &values, &picks)
        }),
    )
    .register(r);

    // `arrayZip(a, b, …)`: `[(a[1], b[1]), …]`; the arrays must be the same
    // length.
    Composite::new(
        "arrayZip",
        Arc::new(|types, _| {
            check_arity("arrayZip", types.len(), 1, usize::MAX)?;
            let mut elems = Vec::with_capacity(types.len());
            for i in 0..types.len() {
                elems.push((None, element_of("arrayZip", types, i)?.clone()));
            }
            Ok(ChType::Array(Box::new(ChType::Tuple(elems))))
        }),
        Arc::new(|c| {
            let list_type = data_type(c.ret)?.0;
            let tuple_type = item_field(&list_type)?.data_type().clone();
            let DataType::Struct(fields) = &tuple_type else {
                return Err(ChError::logical_error(
                    "arrayZip's elements are not a Struct",
                ));
            };
            let parts = c
                .args
                .iter()
                .map(ListParts::of)
                .collect::<Result<Vec<_>, _>>()?;
            let mut lengths = Vec::with_capacity(c.rows);
            for r in 0..c.rows {
                let n = parts[0].len(r);
                if parts.iter().any(|p| p.len(r) != n) {
                    return Err(sizes_dont_match("arrayZip"));
                }
                lengths.push(n);
            }
            let mut columns = Vec::with_capacity(parts.len());
            for (p, f) in parts.iter().zip(fields.iter()) {
                let idx: Vec<u64> = (0..c.rows)
                    .flat_map(|r| p.range(r))
                    .map(|j| j as u64)
                    .collect();
                let col = take_indexes(&p.values, &idx)?;
                columns.push(crate::func::nested::coerce_to(&col, f.data_type())?);
            }
            let values: ArrayRef =
                Arc::new(StructArray::try_new(fields.clone(), columns, None).map_err(arrow_err)?);
            build_list(&list_type, lengths, values)
        }),
    )
    .register(r);

    // `arrayJoin(a)` as a function: typed as its element. Running it needs
    // the row-multiplying operator plan HE1 Task 35 hoists it into; until
    // then a query that reaches it is refused, never answered wrongly.
    Composite::new(
        "arrayJoin",
        Arc::new(|types, _| {
            check_arity("arrayJoin", types.len(), 1, 1)?;
            Ok(element_of("arrayJoin", types, 0)?.clone())
        }),
        Arc::new(|_| {
            Err(ChError::not_implemented(
                "arrayJoin is not supported by the Loams House engine yet (plan HE1 Task 35)",
            ))
        }),
    )
    .own_nulls()
    .register(r);
}

/// Whether `t` is an integer type (after `Nullable` and `LowCardinality`).
pub fn is_integer(t: &ChType) -> bool {
    matches!(
        t.base(),
        ChType::UInt8
            | ChType::UInt16
            | ChType::UInt32
            | ChType::UInt64
            | ChType::Int8
            | ChType::Int16
            | ChType::Int32
            | ChType::Int64
            | ChType::Bool
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_recognised() {
        assert!(is_integer(&ChType::UInt8));
        assert!(is_integer(&ChType::Nullable(Box::new(ChType::Int64))));
        assert!(!is_integer(&ChType::Float64));
        assert!(!is_integer(&ChType::String));
    }
}
