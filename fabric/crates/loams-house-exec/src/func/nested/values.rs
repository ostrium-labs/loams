//! Arrow helpers for the composite-type functions: re-laying an array out in
//! an exact Arrow type (nested field names and `loams.ch.type` metadata
//! included), list parts, and building lists.

use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, ListArray, MapArray, StructArray, UInt32Array, UInt64Array,
    new_empty_array, new_null_array,
};
use datafusion::arrow::buffer::{NullBuffer, OffsetBuffer, ScalarBuffer};
use datafusion::arrow::compute::{cast, concat, take};
use datafusion::arrow::datatypes::{DataType, FieldRef};
use loams_house_types::arrow::data_type;
use loams_house_types::default::default_array;
use loams_house_types::{ChError, ChType};

use super::arrow_err;

/// `a` laid out in exactly `to`: the same values, with `to`'s nested field
/// names, nullability and metadata, converting values where the types
/// differ (`Nullable(Nothing)` to any type, integers to wider ones, values
/// into and out of dictionaries).
pub fn coerce_to(a: &ArrayRef, to: &DataType) -> Result<ArrayRef, ChError> {
    if a.data_type() == to {
        return Ok(Arc::clone(a));
    }
    match (a.data_type(), to) {
        (DataType::Null, _) => Ok(new_null_array(to, a.len())),
        (DataType::List(_), DataType::List(f)) => {
            let l = a.as_list::<i32>();
            let values = coerce_to(l.values(), f.data_type())?;
            Ok(Arc::new(
                ListArray::try_new(
                    Arc::clone(f),
                    l.offsets().clone(),
                    values,
                    l.nulls().cloned(),
                )
                .map_err(arrow_err)?,
            ))
        }
        (DataType::LargeList(item), DataType::List(_)) => {
            let narrowed = cast(a, &DataType::List(Arc::clone(item))).map_err(arrow_err)?;
            coerce_to(&narrowed, to)
        }
        (DataType::Map(..), DataType::Map(f, sorted)) => {
            let m = a.as_map();
            let entries: ArrayRef = Arc::new(m.entries().clone());
            let entries = coerce_to(&entries, f.data_type())?;
            Ok(Arc::new(
                MapArray::try_new(
                    Arc::clone(f),
                    m.offsets().clone(),
                    entries.as_struct().clone(),
                    m.nulls().cloned(),
                    *sorted,
                )
                .map_err(arrow_err)?,
            ))
        }
        (DataType::Struct(from), DataType::Struct(fields)) if from.len() == fields.len() => {
            let s = a.as_struct();
            let columns = s
                .columns()
                .iter()
                .zip(fields.iter())
                .map(|(c, f)| coerce_to(c, f.data_type()))
                .collect::<Result<Vec<_>, _>>()?;
            if fields.is_empty() {
                return Ok(Arc::new(StructArray::new_empty_fields(
                    s.len(),
                    s.nulls().cloned(),
                )));
            }
            Ok(Arc::new(
                StructArray::try_new(fields.clone(), columns, s.nulls().cloned())
                    .map_err(arrow_err)?,
            ))
        }
        (DataType::Dictionary(k1, _), DataType::Dictionary(k2, v2)) if k1 == k2 => {
            let d = a.as_any_dictionary();
            let values = coerce_to(d.values(), v2)?;
            Ok(d.with_values(values))
        }
        (DataType::Dictionary(_, v), _) => {
            let plain = cast(a, v).map_err(arrow_err)?;
            coerce_to(&plain, to)
        }
        (_, DataType::Dictionary(_, v)) => {
            let plain = coerce_to(a, v)?;
            cast(&plain, to).map_err(arrow_err)
        }
        _ => {
            let out = cast(a, to).map_err(arrow_err)?;
            if out.data_type() == to {
                Ok(out)
            } else {
                coerce_to(&out, to)
            }
        }
    }
}

/// `a` with a top-level dictionary decoded to its values' type.
pub fn decode_dictionary(a: &ArrayRef) -> Result<ArrayRef, ChError> {
    match a.data_type() {
        DataType::Dictionary(_, v) => cast(a, v).map_err(arrow_err),
        _ => Ok(Arc::clone(a)),
    }
}

/// `a` converted to ClickHouse type `t`'s layout.
pub fn to_type(a: &ArrayRef, t: &ChType) -> Result<ArrayRef, ChError> {
    coerce_to(a, &data_type(t)?.0)
}

/// An `Array` column's parts: its row offsets into the values, and the
/// values. `LargeList` is narrowed first.
#[derive(Debug, Clone)]
pub struct ListParts {
    /// `rows + 1` offsets; row `r` is `values[offsets[r]..offsets[r + 1]]`.
    pub offsets: Vec<usize>,
    /// Every row's elements, end to end.
    pub values: ArrayRef,
}

impl ListParts {
    /// The parts of the list column `a`.
    pub fn of(a: &ArrayRef) -> Result<Self, ChError> {
        match a.data_type() {
            DataType::List(_) => {
                let l = a.as_list::<i32>();
                Ok(Self {
                    offsets: l.value_offsets().iter().map(|o| *o as usize).collect(),
                    values: Arc::clone(l.values()),
                })
            }
            DataType::LargeList(item) => {
                let narrowed = cast(a, &DataType::List(Arc::clone(item))).map_err(arrow_err)?;
                Self::of(&narrowed)
            }
            DataType::FixedSizeList(item, _) => {
                let narrowed = cast(a, &DataType::List(Arc::clone(item))).map_err(arrow_err)?;
                Self::of(&narrowed)
            }
            DataType::Map(..) => {
                let m = a.as_map();
                Ok(Self {
                    offsets: m.value_offsets().iter().map(|o| *o as usize).collect(),
                    values: Arc::new(m.entries().clone()),
                })
            }
            other => Err(ChError::logical_error(format!(
                "An array column holds an Arrow {other} array"
            ))),
        }
    }

    /// How many rows.
    pub fn rows(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Row `r`'s element range in [`Self::values`].
    pub fn range(&self, r: usize) -> std::ops::Range<usize> {
        self.offsets[r]..self.offsets[r + 1]
    }

    /// Row `r`'s length.
    pub fn len(&self, r: usize) -> usize {
        self.offsets[r + 1] - self.offsets[r]
    }

    /// Whether row `r` is empty.
    pub fn is_empty(&self, r: usize) -> bool {
        self.len(r) == 0
    }

    /// For every element in `values[offsets[0]..offsets[rows]]`, the row it
    /// belongs to.
    pub fn row_of_each_element(&self) -> UInt32Array {
        let mut out = Vec::with_capacity(self.offsets[self.rows()] - self.offsets[0]);
        for r in 0..self.rows() {
            out.extend(std::iter::repeat_n(r as u32, self.len(r)));
        }
        UInt32Array::from(out)
    }

    /// The elements of every row, sliced to the rows' span (a sliced input
    /// has values before its first offset).
    pub fn used_values(&self) -> ArrayRef {
        let start = self.offsets[0];
        let end = self.offsets[self.rows()];
        self.values.slice(start, end - start)
    }
}

/// The item field of an `Array` type's Arrow layout.
pub fn item_field(list_type: &DataType) -> Result<FieldRef, ChError> {
    match list_type {
        DataType::List(f) | DataType::LargeList(f) | DataType::FixedSizeList(f, _) => {
            Ok(Arc::clone(f))
        }
        other => Err(ChError::logical_error(format!(
            "Expected an Arrow list type, got {other}"
        ))),
    }
}

/// A list column of type `list_type` (`List(item)`) from row lengths and the
/// elements end to end.
pub fn build_list(
    list_type: &DataType,
    lengths: impl IntoIterator<Item = usize>,
    values: ArrayRef,
) -> Result<ArrayRef, ChError> {
    let field = item_field(list_type)?;
    let values = coerce_to(&values, field.data_type())?;
    let offsets = OffsetBuffer::<i32>::from_lengths(lengths);
    Ok(Arc::new(
        ListArray::try_new(field, offsets, values, None).map_err(arrow_err)?,
    ))
}

/// A list column of type `list_type` whose row `r` holds
/// `values[ranges[r]]` for each of its index lists: `picks[r]` are indexes
/// into `values`.
pub fn gather_list(
    list_type: &DataType,
    values: &ArrayRef,
    picks: &[Vec<u64>],
) -> Result<ArrayRef, ChError> {
    let lengths: Vec<usize> = picks.iter().map(Vec::len).collect();
    let flat: Vec<u64> = picks.iter().flatten().copied().collect();
    let taken = take_indexes(values, &flat)?;
    build_list(list_type, lengths, taken)
}

/// `values` at `indexes`.
pub fn take_indexes(values: &ArrayRef, indexes: &[u64]) -> Result<ArrayRef, ChError> {
    if indexes.is_empty() {
        return Ok(new_empty_array(values.data_type()));
    }
    let idx = UInt64Array::from(indexes.to_vec());
    take(values.as_ref(), &idx, None).map_err(arrow_err)
}

/// `values` followed by one default value of `t`: an index of `values.len()`
/// into the result picks the default (what `arrayElement` answers out of
/// range).
pub fn with_default(values: &ArrayRef, t: &ChType) -> Result<ArrayRef, ChError> {
    let default = default_array(t)?;
    let default = coerce_to(&default, values.data_type())?;
    concat(&[values.as_ref(), default.as_ref()]).map_err(arrow_err)
}

/// A one-row-per-value validity buffer: `None` when every value is valid.
pub fn nulls_from(valid: &[bool]) -> Option<NullBuffer> {
    if valid.iter().all(|v| *v) {
        None
    } else {
        Some(NullBuffer::from(valid.to_vec()))
    }
}

/// Offsets from row lengths, as `i32`.
pub fn offsets_from(lengths: &[usize]) -> OffsetBuffer<i32> {
    let mut out = Vec::with_capacity(lengths.len() + 1);
    let mut at = 0i32;
    out.push(0);
    for l in lengths {
        at += *l as i32;
        out.push(at);
    }
    OffsetBuffer::new(ScalarBuffer::from(out))
}

/// An integer column's values, `NULL` as `None`. Booleans read as 0 and 1.
pub fn ints(a: &ArrayRef) -> Result<Vec<Option<i128>>, ChError> {
    use datafusion::arrow::datatypes as t;
    let a = decode_dictionary(a)?;
    let n = a.len();
    let get = |valid: &dyn Fn(usize) -> bool, v: &dyn Fn(usize) -> i128| -> Vec<Option<i128>> {
        (0..n).map(|i| valid(i).then(|| v(i))).collect()
    };
    let valid = |i: usize| a.is_valid(i);
    Ok(match a.data_type() {
        DataType::Null => vec![None; n],
        DataType::Boolean => {
            let b = a.as_boolean();
            get(&valid, &|i| i128::from(b.value(i)))
        }
        DataType::Int8 => {
            let p = a.as_primitive::<t::Int8Type>();
            get(&valid, &|i| i128::from(p.value(i)))
        }
        DataType::Int16 => {
            let p = a.as_primitive::<t::Int16Type>();
            get(&valid, &|i| i128::from(p.value(i)))
        }
        DataType::Int32 => {
            let p = a.as_primitive::<t::Int32Type>();
            get(&valid, &|i| i128::from(p.value(i)))
        }
        DataType::Int64 => {
            let p = a.as_primitive::<t::Int64Type>();
            get(&valid, &|i| i128::from(p.value(i)))
        }
        DataType::UInt8 => {
            let p = a.as_primitive::<t::UInt8Type>();
            get(&valid, &|i| i128::from(p.value(i)))
        }
        DataType::UInt16 => {
            let p = a.as_primitive::<t::UInt16Type>();
            get(&valid, &|i| i128::from(p.value(i)))
        }
        DataType::UInt32 => {
            let p = a.as_primitive::<t::UInt32Type>();
            get(&valid, &|i| i128::from(p.value(i)))
        }
        DataType::UInt64 => {
            let p = a.as_primitive::<t::UInt64Type>();
            get(&valid, &|i| i128::from(p.value(i)))
        }
        other => {
            return Err(ChError::illegal_type_of_argument(format!(
                "Expected an integer column, got Arrow {other}"
            )));
        }
    })
}

/// Whether a column's value is "true" as ClickHouse reads a condition: not
/// `NULL` and not zero.
pub fn truths(a: &ArrayRef) -> Result<Vec<bool>, ChError> {
    let a = decode_dictionary(a)?;
    if let DataType::Float32 | DataType::Float64 = a.data_type() {
        let f = cast(&a, &DataType::Float64).map_err(arrow_err)?;
        let f = f.as_primitive::<datafusion::arrow::datatypes::Float64Type>();
        return Ok((0..f.len())
            .map(|i| f.is_valid(i) && f.value(i) != 0.0)
            .collect());
    }
    Ok(ints(&a)?
        .into_iter()
        .map(|v| v.is_some_and(|v| v != 0))
        .collect())
}

/// Whether element `i` of `a` is `NULL` (dictionaries looked through).
pub fn is_null_at(a: &dyn Array, i: usize) -> bool {
    a.logical_nulls().is_some_and(|n| n.is_null(i))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use datafusion::arrow::array::{Int64Array, UInt8Array};
    use loams_house_types::arrow::to_arrow;

    #[test]
    fn lists_take_the_exact_nested_layout() {
        let t = ChType::Array(Box::new(ChType::UInt8));
        let target = to_arrow("x", &t).unwrap().data_type().clone();
        let plain = build_list(
            &DataType::new_list(DataType::Int64, true),
            [2, 0, 1],
            Arc::new(Int64Array::from(vec![1, 2, 3])),
        )
        .unwrap();
        let out = coerce_to(&plain, &target).unwrap();
        assert_eq!(out.data_type(), &target);
        let parts = ListParts::of(&out).unwrap();
        assert_eq!(parts.offsets, vec![0, 2, 2, 3]);
        assert_eq!(
            parts
                .values
                .as_primitive::<datafusion::arrow::datatypes::UInt8Type>(),
            &UInt8Array::from(vec![1, 2, 3])
        );
        assert_eq!(
            parts.row_of_each_element(),
            UInt32Array::from(vec![0, 0, 2])
        );
    }

    #[test]
    fn a_default_follows_the_values() {
        let values: ArrayRef = Arc::new(UInt8Array::from(vec![7]));
        let out = with_default(&values, &ChType::UInt8).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(
            out.as_primitive::<datafusion::arrow::datatypes::UInt8Type>()
                .value(1),
            0
        );
    }
}
