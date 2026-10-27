//! Value equality and order for the composite-type functions, on Arrow's row
//! format: `has`, `indexOf`, `arrayDistinct`, `arrayEnumerateUniq`, map key
//! lookups and `arraySort` compare encoded rows, which works the same for
//! every element type (strings, numbers, tuples, nested arrays, dictionaries).
//! `NULL` equals `NULL`, as ClickHouse's `has([NULL], NULL)` is 1.

use datafusion::arrow::array::ArrayRef;
use datafusion::arrow::compute::SortOptions;
use datafusion::arrow::row::{RowConverter, Rows, SortField};
use loams_house_types::{ChError, ChType};

use super::supertype::supertype;
use super::values::to_type;

/// Row-format keys of `arrays`, which share one Arrow type, comparable with
/// each other.
pub fn keys(arrays: &[ArrayRef], options: SortOptions) -> Result<Vec<Rows>, ChError> {
    let Some(first) = arrays.first() else {
        return Ok(Vec::new());
    };
    let dt = first.data_type().clone();
    let fields = vec![SortField::new_with_options(dt.clone(), options)];
    if !RowConverter::supports_fields(&fields) {
        return Err(ChError::unsupported(format!(
            "Comparing values of Arrow type {dt}"
        )));
    }
    let converter = RowConverter::new(fields).map_err(super::arrow_err)?;
    arrays
        .iter()
        .map(|a| {
            converter
                .convert_columns(std::slice::from_ref(a))
                .map_err(super::arrow_err)
        })
        .collect()
}

/// Keys for equality only.
pub fn eq_keys(arrays: &[ArrayRef]) -> Result<Vec<Rows>, ChError> {
    keys(arrays, SortOptions::default())
}

/// `t` without a top-level `LowCardinality`.
fn plain(t: ChType) -> ChType {
    match t {
        ChType::LowCardinality(inner) => *inner,
        other => other,
    }
}

/// The type two sides are compared in: their supertype (`386` when there is
/// none), without a top-level `LowCardinality`.
pub fn comparison_type(a: &ChType, b: &ChType) -> Result<ChType, ChError> {
    Ok(plain(supertype(&[a.clone(), b.clone()])?))
}

/// `a` (of type `at`) and `b` (of type `bt`) in their comparison type, ready
/// for [`eq_keys`].
pub fn comparable(
    a: &ArrayRef,
    at: &ChType,
    b: &ArrayRef,
    bt: &ChType,
) -> Result<(ArrayRef, ArrayRef), ChError> {
    let t = comparison_type(at, bt)?;
    Ok((to_type(a, &t)?, to_type(b, &t)?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use datafusion::arrow::array::{Int64Array, StringArray, UInt8Array};

    use super::*;

    #[test]
    fn null_equals_null_and_types_meet() {
        let a: ArrayRef = Arc::new(UInt8Array::from(vec![Some(1), None]));
        let b: ArrayRef = Arc::new(Int64Array::from(vec![Some(1), None]));
        let (a, b) = comparable(
            &a,
            &ChType::Nullable(Box::new(ChType::UInt8)),
            &b,
            &ChType::Nullable(Box::new(ChType::Int64)),
        )
        .unwrap();
        let k = eq_keys(&[a, b]).unwrap();
        assert!(k[0].row(0) == k[1].row(0));
        assert!(k[0].row(1) == k[1].row(1));
        assert!(k[0].row(0) != k[1].row(1));
    }

    #[test]
    fn strings_order_by_bytes() {
        let a: ArrayRef = Arc::new(StringArray::from(vec!["b", "a", "B"]));
        let k = keys(&[a], SortOptions::default()).unwrap();
        assert!(k[0].row(2) < k[0].row(1));
        assert!(k[0].row(1) < k[0].row(0));
    }
}
