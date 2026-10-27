//! Functions that reshape arrays: `arraySlice`, `arrayDistinct`,
//! `arrayEnumerate`, `arrayEnumerateUniq`, `arrayReverse` and
//! `arrayStringConcat` (ClickHouse `src/Functions/array/arraySlice.cpp`,
//! `GatherUtils/Algorithms.h`, `arrayDistinct.cpp`, `arrayEnumerate.cpp`,
//! `arrayEnumerateExtended.h`, `arrayReverse.cpp`, `arrayStringConcat.cpp`,
//! Apache-2.0).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, StringArray, UInt32Array};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::DataType;
use loams_house_sql::Registry;
use loams_house_types::arrow::data_type;
use loams_house_types::text::{Style, TextSettings, write_value};
use loams_house_types::{ChError, ChType};

use super::build::is_integer;
use crate::func::nested::rows::eq_keys;
use crate::func::nested::values::{ListParts, build_list, gather_list, ints, is_null_at};
use crate::func::nested::{
    Composite, arrow_err, check_arity, element_of, illegal_type, sizes_dont_match,
};

/// Registers the reshaping functions.
pub fn register(r: &mut Registry) {
    // `arraySlice(a, offset[, length])`. A `NULL` offset reads as 1 and a
    // `NULL` length as "to the end", as ClickHouse's GatherUtils do.
    Composite::new(
        "arraySlice",
        Arc::new(|types, _| {
            check_arity("arraySlice", types.len(), 2, 3)?;
            element_of("arraySlice", types, 0)?;
            for (i, t) in types.iter().enumerate().skip(1) {
                if !is_integer(t) && !matches!(t.base(), ChType::Nothing) {
                    return Err(illegal_type("arraySlice", i, t));
                }
            }
            Ok(types[0].clone())
        }),
        Arc::new(|c| {
            let parts = ListParts::of(&c.args[0])?;
            let offsets = ints(&c.args[1])?;
            let lengths = match c.args.get(2) {
                Some(a) => Some(ints(a)?),
                None => None,
            };
            let picks: Vec<Vec<u64>> = (0..c.rows)
                .map(|r| {
                    let range = slice(
                        parts.len(r),
                        offsets[r].unwrap_or(1),
                        lengths.as_ref().and_then(|l| l[r]),
                    );
                    let base = parts.offsets[r] as u64;
                    range.map(|j| base + j as u64).collect()
                })
                .collect();
            gather_list(&data_type(c.ret)?.0, &parts.values, &picks)
        }),
    )
    .own_nulls()
    .register(r);

    // `arrayDistinct(a)`: the first occurrence of each value, `NULL`s left
    // out.
    Composite::new(
        "arrayDistinct",
        Arc::new(|types, _| {
            check_arity("arrayDistinct", types.len(), 1, 1)?;
            element_of("arrayDistinct", types, 0)?;
            Ok(types[0].clone())
        }),
        Arc::new(|c| {
            let parts = ListParts::of(&c.args[0])?;
            let k = eq_keys(std::slice::from_ref(&parts.values))?;
            let picks: Vec<Vec<u64>> = (0..c.rows)
                .map(|r| {
                    let mut seen = HashSet::new();
                    parts
                        .range(r)
                        .filter(|j| !is_null_at(parts.values.as_ref(), *j))
                        .filter(|j| seen.insert(k[0].row(*j).data()))
                        .map(|j| j as u64)
                        .collect()
                })
                .collect();
            gather_list(&data_type(c.ret)?.0, &parts.values, &picks)
        }),
    )
    .register(r);

    // `arrayEnumerate(a)`: `[1, 2, …, length(a)]`.
    Composite::new(
        "arrayEnumerate",
        Arc::new(|types, _| {
            check_arity("arrayEnumerate", types.len(), 1, 1)?;
            element_of("arrayEnumerate", types, 0)?;
            Ok(ChType::Array(Box::new(ChType::UInt32)))
        }),
        Arc::new(|c| {
            let parts = ListParts::of(&c.args[0])?;
            let lengths: Vec<usize> = (0..c.rows).map(|r| parts.len(r)).collect();
            let values: Vec<u32> = lengths.iter().flat_map(|n| 1..=*n as u32).collect();
            build_list(
                &data_type(c.ret)?.0,
                lengths,
                Arc::new(UInt32Array::from(values)),
            )
        }),
    )
    .register(r);

    // `arrayEnumerateUniq(a, …)`: for each element, how many times its value
    // (the tuple of the arrays' elements at that position) has appeared so
    // far, itself included. `NULL` is a value.
    Composite::new(
        "arrayEnumerateUniq",
        Arc::new(|types, _| {
            check_arity("arrayEnumerateUniq", types.len(), 1, usize::MAX)?;
            for i in 0..types.len() {
                element_of("arrayEnumerateUniq", types, i)?;
            }
            Ok(ChType::Array(Box::new(ChType::UInt32)))
        }),
        Arc::new(|c| {
            let parts = c
                .args
                .iter()
                .map(ListParts::of)
                .collect::<Result<Vec<_>, _>>()?;
            let keys = parts
                .iter()
                .map(|p| eq_keys(std::slice::from_ref(&p.values)).map(|mut k| k.remove(0)))
                .collect::<Result<Vec<_>, _>>()?;
            let mut lengths = Vec::with_capacity(c.rows);
            let mut values = Vec::new();
            for r in 0..c.rows {
                let n = parts[0].len(r);
                if parts.iter().any(|p| p.len(r) != n) {
                    return Err(sizes_dont_match("arrayEnumerateUniq"));
                }
                lengths.push(n);
                let mut counts: HashMap<Vec<u8>, u32> = HashMap::new();
                for e in 0..n {
                    let mut key = Vec::new();
                    for (p, k) in parts.iter().zip(&keys) {
                        key.extend_from_slice(k.row(p.offsets[r] + e).data());
                    }
                    let count = counts.entry(key).or_insert(0);
                    *count += 1;
                    values.push(*count);
                }
            }
            build_list(
                &data_type(c.ret)?.0,
                lengths,
                Arc::new(UInt32Array::from(values)),
            )
        }),
    )
    .register(r);

    // `arrayReverse(a)`.
    Composite::new(
        "arrayReverse",
        Arc::new(|types, _| {
            check_arity("arrayReverse", types.len(), 1, 1)?;
            element_of("arrayReverse", types, 0)?;
            Ok(types[0].clone())
        }),
        Arc::new(|c| {
            let parts = ListParts::of(&c.args[0])?;
            let picks: Vec<Vec<u64>> = (0..c.rows)
                .map(|r| parts.range(r).rev().map(|j| j as u64).collect())
                .collect();
            gather_list(&data_type(c.ret)?.0, &parts.values, &picks)
        }),
    )
    .register(r);

    // `arrayStringConcat(a[, separator])`: the elements as text, `NULL`s
    // left out, joined by the constant separator ('' by default).
    Composite::new(
        "arrayStringConcat",
        Arc::new(|types, scalars| {
            check_arity("arrayStringConcat", types.len(), 1, 2)?;
            element_of("arrayStringConcat", types, 0)?;
            if let Some(t) = types.get(1) {
                if !matches!(t, ChType::String | ChType::FixedString(_)) {
                    return Err(illegal_type("arrayStringConcat", 1, t));
                }
                if scalars.get(1).is_none_or(Option::is_none) {
                    return Err(ChError::illegal_type_of_argument(
                        "Second argument of function arrayStringConcat must be a constant string",
                    ));
                }
            }
            Ok(ChType::String)
        }),
        Arc::new(|c| {
            let parts = ListParts::of(&c.args[0])?;
            let elem = element_of(c.name, c.types, 0)?;
            let separator = match c.consts.get(1).and_then(Option::as_ref) {
                Some(s) => match s {
                    datafusion::common::ScalarValue::Utf8(Some(v))
                    | datafusion::common::ScalarValue::LargeUtf8(Some(v))
                    | datafusion::common::ScalarValue::Utf8View(Some(v)) => v.clone(),
                    datafusion::common::ScalarValue::FixedSizeBinary(_, Some(b)) => {
                        String::from_utf8_lossy(b).into_owned()
                    }
                    _ => String::new(),
                },
                None => String::new(),
            };
            let texts = element_texts(&parts.values, elem)?;
            let out: Vec<String> = (0..c.rows)
                .map(|r| {
                    parts
                        .range(r)
                        .filter_map(|j| texts[j].as_deref())
                        .collect::<Vec<_>>()
                        .join(&separator)
                })
                .collect();
            Ok(Arc::new(StringArray::from(out)) as ArrayRef)
        }),
    )
    .register(r);
}

/// The element positions `arraySlice` keeps of an array of `size` elements
/// (ClickHouse's `sliceDynamicOffset{Unbounded,Bounded}`: `getSliceFromLeft`
/// and `getSliceFromRight`).
pub fn slice(size: usize, offset: i128, length: Option<i128>) -> std::ops::Range<usize> {
    let size_i = size as i128;
    let empty = 0..0;
    match length {
        None => {
            if offset > 0 {
                let from = offset - 1;
                if from >= size_i {
                    empty
                } else {
                    from as usize..size
                }
            } else if offset < 0 {
                let back = -offset;
                if back > size_i {
                    0..size
                } else {
                    (size_i - back) as usize..size
                }
            } else {
                empty
            }
        }
        Some(mut length) => {
            if length < 0 {
                length += if offset > 0 {
                    size_i - (offset - 1)
                } else {
                    -offset
                };
            }
            if offset == 0 || length <= 0 {
                return empty;
            }
            if offset > 0 {
                let from = offset - 1;
                if from >= size_i {
                    return empty;
                }
                let n = length.min(size_i - from);
                from as usize..(from + n) as usize
            } else {
                let back = -offset;
                if back > size_i {
                    let n = if length + size_i > back {
                        (length + size_i - back).min(size_i)
                    } else {
                        0
                    };
                    0..n as usize
                } else {
                    let from = size_i - back;
                    let n = length.min(back);
                    from as usize..(from + n) as usize
                }
            }
        }
    }
}

/// Each element as `toString` writes it, `None` for `NULL`.
fn element_texts(values: &ArrayRef, elem: &ChType) -> Result<Vec<Option<String>>, ChError> {
    match elem.base() {
        ChType::String | ChType::Json(_) => {
            let s = cast(values, &DataType::Utf8).map_err(arrow_err)?;
            let s = s.as_string::<i32>();
            Ok((0..s.len())
                .map(|i| s.is_valid(i).then(|| s.value(i).to_string()))
                .collect())
        }
        ChType::FixedString(_) => {
            let plain = crate::func::nested::values::decode_dictionary(values)?;
            let f = plain.as_fixed_size_binary();
            Ok((0..f.len())
                .map(|i| {
                    f.is_valid(i)
                        .then(|| String::from_utf8_lossy(f.value(i)).into_owned())
                })
                .collect())
        }
        base => {
            let style = if matches!(base, ChType::Array(_) | ChType::Map(..) | ChType::Tuple(_)) {
                Style::Quoted
            } else {
                Style::Tsv
            };
            let settings = TextSettings::default();
            (0..values.len())
                .map(|i| {
                    if is_null_at(values.as_ref(), i) {
                        return Ok(None);
                    }
                    let mut out = Vec::new();
                    write_value(elem, values.as_ref(), i, style, &settings, &mut out)?;
                    Ok(Some(String::from_utf8_lossy(&out).into_owned()))
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::slice;

    #[test]
    fn slices_follow_gather_utils() {
        // arraySlice([1, 2, 3, 4, 5], …)
        assert_eq!(slice(5, 2, None), 1..5);
        assert_eq!(slice(5, -2, None), 3..5);
        assert_eq!(slice(5, 0, None), 0..0);
        assert_eq!(slice(5, 9, None), 0..0);
        assert_eq!(slice(5, -9, None), 0..5);
        assert_eq!(slice(5, 2, Some(2)), 1..3);
        assert_eq!(slice(5, 2, Some(-1)), 1..4);
        assert_eq!(slice(5, -3, Some(2)), 2..4);
        assert_eq!(slice(5, -7, Some(3)), 0..1);
        assert_eq!(slice(5, 1, Some(0)), 0..0);
        assert_eq!(slice(0, 1, Some(3)), 0..0);
    }
}
