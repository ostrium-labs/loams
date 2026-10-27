//! The work behind `arraySum` and `arraySort`/`arrayReverseSort`, with and
//! without a lambda (ClickHouse `src/Functions/array/arrayAggregation.cpp`,
//! `arraySort.cpp`, Apache-2.0). The functions themselves are higher-order
//! functions ([`crate::func::lambda`]), because DataFusion prefers a scalar
//! function of the same name over a higher-order one.

use std::cmp::Ordering;
use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, Decimal128Array, Float64Array, Int64Array, UInt64Array,
};
use datafusion::arrow::compute::{SortOptions, cast};
use datafusion::arrow::datatypes::{DataType, Float64Type, Int64Type, UInt64Type};
use loams_house_types::{ChError, ChType};

use crate::func::nested::rows::keys;
use crate::func::nested::values::{decode_dictionary, is_null_at};
use crate::func::nested::{arrow_err, illegal_type};

/// The type `arraySum` answers for elements (or lambda results) of type `t`:
/// `Int64` for signed integers, `UInt64` for unsigned ones and `Bool`,
/// `Float64` for floats and `Decimal(38, S)` for decimals up to 38 digits.
/// `Nullable` elements are `43`, as in ClickHouse.
pub fn sum_type(name: &str, t: &ChType) -> Result<ChType, ChError> {
    Ok(match t {
        ChType::LowCardinality(inner) => return sum_type(name, inner),
        ChType::Int8 | ChType::Int16 | ChType::Int32 | ChType::Int64 => ChType::Int64,
        ChType::UInt8 | ChType::UInt16 | ChType::UInt32 | ChType::UInt64 | ChType::Bool => {
            ChType::UInt64
        }
        ChType::Float32 | ChType::Float64 => ChType::Float64,
        ChType::Decimal { precision, scale } if *precision <= 38 => ChType::Decimal {
            precision: 38,
            scale: *scale,
        },
        other => {
            return Err(ChError::illegal_type_of_argument(format!(
                "{name} cannot add values of type {other}"
            )));
        }
    })
}

/// Sums `values` (of type `t`) over the element ranges `rows` (relative
/// offsets, `rows.len() - 1` rows), wrapping on overflow as ClickHouse does.
pub fn sum(
    name: &str,
    values: &ArrayRef,
    t: &ChType,
    offsets: &[usize],
) -> Result<ArrayRef, ChError> {
    let out_t = sum_type(name, t)?;
    let values = decode_dictionary(values)?;
    let n = offsets.len().saturating_sub(1);
    let ranges = (0..n).map(|r| offsets[r]..offsets[r + 1]);
    Ok(match out_t {
        ChType::Int64 => {
            let v = cast(&values, &DataType::Int64).map_err(arrow_err)?;
            let v = v.as_primitive::<Int64Type>();
            Arc::new(Int64Array::from(
                ranges
                    .map(|r| r.fold(0i64, |acc, j| acc.wrapping_add(v.value(j))))
                    .collect::<Vec<_>>(),
            ))
        }
        ChType::UInt64 => {
            let v = cast(&values, &DataType::UInt64).map_err(arrow_err)?;
            let v = v.as_primitive::<UInt64Type>();
            Arc::new(UInt64Array::from(
                ranges
                    .map(|r| r.fold(0u64, |acc, j| acc.wrapping_add(v.value(j))))
                    .collect::<Vec<_>>(),
            ))
        }
        ChType::Float64 => {
            let v = cast(&values, &DataType::Float64).map_err(arrow_err)?;
            let v = v.as_primitive::<Float64Type>();
            Arc::new(Float64Array::from(
                ranges
                    .map(|r| r.fold(0f64, |acc, j| acc + v.value(j)))
                    .collect::<Vec<_>>(),
            ))
        }
        ChType::Decimal { precision, scale } => {
            let dt = DataType::Decimal128(precision, scale as i8);
            let v = cast(&values, &dt).map_err(arrow_err)?;
            let v = v.as_primitive::<datafusion::arrow::datatypes::Decimal128Type>();
            Arc::new(
                Decimal128Array::from(
                    ranges
                        .map(|r| r.fold(0i128, |acc, j| acc.wrapping_add(v.value(j))))
                        .collect::<Vec<_>>(),
                )
                .with_precision_and_scale(precision, scale as i8)
                .map_err(arrow_err)?,
            )
        }
        other => return Err(illegal_type(name, 0, &other)),
    })
}

/// For each row (relative `offsets` into `sort_keys`), the order of its
/// elements: ascending, or descending for `arrayReverseSort`; a float NaN
/// after the numbers and `NULL` last either way, as ClickHouse sorts.
/// Elements with equal keys keep their order.
pub fn sort_order(
    sort_keys: &ArrayRef,
    offsets: &[usize],
    descending: bool,
) -> Result<Vec<Vec<usize>>, ChError> {
    let plain = decode_dictionary(sort_keys)?;
    let nan: Vec<bool> = match plain.data_type() {
        DataType::Float32 | DataType::Float64 => {
            let f = cast(&plain, &DataType::Float64).map_err(arrow_err)?;
            let f = f.as_primitive::<Float64Type>();
            (0..f.len())
                .map(|i| f.is_valid(i) && f.value(i).is_nan())
                .collect()
        }
        _ => vec![false; plain.len()],
    };
    let rows = keys(
        std::slice::from_ref(sort_keys),
        SortOptions {
            descending,
            nulls_first: false,
        },
    )?;
    let rows = &rows[0];
    let class = |j: usize| -> u8 {
        if is_null_at(plain.as_ref(), j) {
            2
        } else if nan[j] {
            1
        } else {
            0
        }
    };
    let n = offsets.len().saturating_sub(1);
    Ok((0..n)
        .map(|r| {
            let mut order: Vec<usize> = (offsets[r]..offsets[r + 1]).collect();
            order.sort_by(|a, b| match class(*a).cmp(&class(*b)) {
                Ordering::Equal if class(*a) == 0 => rows.row(*a).cmp(&rows.row(*b)),
                other => other,
            });
            order
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn sums_widen_and_wrap() {
        let v: ArrayRef = Arc::new(datafusion::arrow::array::UInt8Array::from(vec![
            200, 100, 7,
        ]));
        let out = sum("arraySum", &v, &ChType::UInt8, &[0, 2, 2, 3]).unwrap();
        let out = out.as_primitive::<UInt64Type>();
        assert_eq!(out.values().to_vec(), vec![300, 0, 7]);
        assert_eq!(
            sum_type("arraySum", &ChType::Nullable(Box::new(ChType::UInt8)))
                .unwrap_err()
                .code,
            43
        );
    }

    #[test]
    fn sorts_put_nan_then_null_last() {
        let v: ArrayRef = Arc::new(Float64Array::from(vec![
            Some(2.0),
            None,
            Some(f64::NAN),
            Some(1.0),
        ]));
        assert_eq!(
            sort_order(&v, &[0, 4], false).unwrap(),
            vec![vec![3, 0, 2, 1]]
        );
        assert_eq!(
            sort_order(&v, &[0, 4], true).unwrap(),
            vec![vec![0, 3, 2, 1]]
        );
    }
}
