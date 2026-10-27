//! Element access and membership: `arrayElement` (`a[i]`, `m[k]`), `has`,
//! `hasAll`, `hasAny`, `indexOf`, `length`, `empty` and `notEmpty`
//! (ClickHouse `src/Functions/array/arrayElement.cpp`, `has.cpp`,
//! `hasAll.cpp`, `hasAny.cpp`, `arrayIndex.h`, `length.cpp`, `empty.cpp`,
//! Apache-2.0).
//!
//! `arrayElement` counts from 1, from the end when negative, and answers the
//! element type's default (`NULL` for a `Nullable` element) out of range; on a
//! map it answers the value of the first entry with the key, or the value
//! type's default.

use std::collections::HashSet;
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, UInt8Array, UInt64Array};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::DataType;
use loams_house_sql::Registry;
use loams_house_types::{ChError, ChType};

use super::build::is_integer;
use crate::func::nested::rows::{comparable, comparison_type, eq_keys};
use crate::func::nested::values::{ListParts, ints, take_indexes, with_default};
use crate::func::nested::{Call, Composite, arrow_err, check_arity, element_of, illegal_type};

/// Registers the access and membership functions.
pub fn register(r: &mut Registry) {
    Composite::new(
        "arrayElement",
        Arc::new(|types, _| {
            check_arity("arrayElement", types.len(), 2, 2)?;
            match &types[0] {
                ChType::Array(elem) => {
                    if !is_integer(&types[1]) {
                        return Err(illegal_type("arrayElement", 1, &types[1]));
                    }
                    Ok((**elem).clone())
                }
                ChType::Map(k, v) => {
                    comparison_type(k, &types[1])?;
                    Ok((**v).clone())
                }
                other => Err(illegal_type("arrayElement", 0, other)),
            }
        }),
        Arc::new(|c| match &c.types[0] {
            ChType::Array(elem) => array_element(c, elem),
            ChType::Map(k, v) => map_element(c, k, v),
            other => Err(illegal_type("arrayElement", 0, other)),
        }),
    )
    .register(r);

    Composite::new(
        "has",
        Arc::new(|types, _| {
            check_arity("has", types.len(), 2, 2)?;
            let elem = element_of("has", types, 0)?;
            comparison_type(elem, &types[1])?;
            Ok(ChType::UInt8)
        }),
        Arc::new(|c| {
            let found = first_match(c)?;
            Ok(Arc::new(UInt8Array::from(
                found
                    .iter()
                    .map(|f| u8::from(f.is_some()))
                    .collect::<Vec<_>>(),
            )))
        }),
    )
    .own_nulls()
    .register(r);

    Composite::new(
        "indexOf",
        Arc::new(|types, _| {
            check_arity("indexOf", types.len(), 2, 2)?;
            let elem = element_of("indexOf", types, 0)?;
            comparison_type(elem, &types[1])?;
            Ok(ChType::UInt64)
        }),
        Arc::new(|c| {
            let found = first_match(c)?;
            Ok(Arc::new(UInt64Array::from(
                found
                    .iter()
                    .map(|f| f.map_or(0, |i| i as u64 + 1))
                    .collect::<Vec<_>>(),
            )))
        }),
    )
    .own_nulls()
    .register(r);

    for (name, all) in [("hasAll", true), ("hasAny", false)] {
        Composite::new(
            name,
            Arc::new(move |types, _| {
                check_arity(name, types.len(), 2, 2)?;
                let a = element_of(name, types, 0)?;
                let b = element_of(name, types, 1)?;
                comparison_type(a, b)?;
                Ok(ChType::UInt8)
            }),
            Arc::new(move |c| {
                let a = ListParts::of(&c.args[0])?;
                let b = ListParts::of(&c.args[1])?;
                let at = element_of(name, c.types, 0)?;
                let bt = element_of(name, c.types, 1)?;
                let (av, bv) = comparable(&a.values, at, &b.values, bt)?;
                let k = eq_keys(&[av, bv])?;
                let mut out = Vec::with_capacity(c.rows);
                for r in 0..c.rows {
                    let set: HashSet<&[u8]> = a.range(r).map(|j| k[0].row(j).data()).collect();
                    let mut members = b.range(r).map(|j| set.contains(k[1].row(j).data()));
                    let v = if all {
                        members.all(|m| m)
                    } else {
                        members.any(|m| m)
                    };
                    out.push(u8::from(v));
                }
                Ok(Arc::new(UInt8Array::from(out)) as ArrayRef)
            }),
        )
        .own_nulls()
        .register(r);
    }

    Composite::new(
        "length",
        Arc::new(|types, _| {
            check_arity("length", types.len(), 1, 1)?;
            match &types[0] {
                ChType::String | ChType::FixedString(_) | ChType::Array(_) | ChType::Map(..) => {
                    Ok(ChType::UInt64)
                }
                other => Err(illegal_type("length", 0, other)),
            }
        }),
        Arc::new(|c| {
            let lens = lengths(c)?;
            Ok(Arc::new(UInt64Array::from(
                lens.into_iter().map(|l| l as u64).collect::<Vec<_>>(),
            )))
        }),
    )
    .register(r);

    for (name, want_empty) in [("empty", true), ("notEmpty", false)] {
        Composite::new(
            name,
            Arc::new(move |types, _| {
                check_arity(name, types.len(), 1, 1)?;
                match &types[0] {
                    ChType::String
                    | ChType::FixedString(_)
                    | ChType::Array(_)
                    | ChType::Map(..)
                    | ChType::Uuid => Ok(ChType::UInt8),
                    other => Err(illegal_type(name, 0, other)),
                }
            }),
            Arc::new(move |c| {
                let empty: Vec<bool> = match &c.types[0] {
                    ChType::FixedString(_) | ChType::Uuid => {
                        let a = cast_fixed(&c.args[0])?;
                        let f = a.as_fixed_size_binary();
                        (0..f.len())
                            .map(|i| f.is_null(i) || f.value(i).iter().all(|b| *b == 0))
                            .collect()
                    }
                    _ => lengths(c)?.into_iter().map(|l| l == 0).collect(),
                };
                Ok(Arc::new(UInt8Array::from(
                    empty
                        .into_iter()
                        .map(|e| u8::from(e == want_empty))
                        .collect::<Vec<_>>(),
                )) as ArrayRef)
            }),
        )
        .register(r);
    }
}

fn cast_fixed(a: &ArrayRef) -> Result<ArrayRef, ChError> {
    match a.data_type() {
        DataType::FixedSizeBinary(_) => Ok(Arc::clone(a)),
        other => Err(ChError::logical_error(format!(
            "A FixedString column holds an Arrow {other} array"
        ))),
    }
}

/// The length of each row of a `String`, `FixedString`, `Array` or `Map`
/// argument (bytes for strings).
fn lengths(c: &Call<'_>) -> Result<Vec<usize>, ChError> {
    let a = &c.args[0];
    Ok(match &c.types[0] {
        ChType::String => {
            let s = cast(a, &DataType::Utf8).map_err(arrow_err)?;
            let s = s.as_string::<i32>();
            (0..s.len())
                .map(|i| if s.is_null(i) { 0 } else { s.value(i).len() })
                .collect()
        }
        ChType::FixedString(n) => vec![*n as usize; c.rows],
        ChType::Array(_) | ChType::Map(..) => {
            let p = ListParts::of(a)?;
            (0..p.rows()).map(|r| p.len(r)).collect()
        }
        other => return Err(illegal_type(c.name, 0, other)),
    })
}

/// For each row, the position (from 0) of the first element of argument 0
/// equal to argument 1.
fn first_match(c: &Call<'_>) -> Result<Vec<Option<usize>>, ChError> {
    let parts = ListParts::of(&c.args[0])?;
    let elem = element_of(c.name, c.types, 0)?;
    let (values, needles) = comparable(&parts.values, elem, &c.args[1], &c.types[1])?;
    let k = eq_keys(&[values, needles])?;
    Ok((0..c.rows)
        .map(|r| {
            let needle = k[1].row(r);
            parts.range(r).position(|j| k[0].row(j) == needle)
        })
        .collect())
}

fn array_element(c: &Call<'_>, elem: &ChType) -> Result<ArrayRef, ChError> {
    let parts = ListParts::of(&c.args[0])?;
    let index = ints(&c.args[1])?;
    let default_at = parts.values.len() as u64;
    let picks: Vec<u64> = (0..c.rows)
        .map(|r| {
            let n = parts.len(r) as i128;
            let start = parts.offsets[r] as i128;
            match index[r] {
                Some(i) if i > 0 && i <= n => (start + i - 1) as u64,
                Some(i) if i < 0 && -i <= n => (start + n + i) as u64,
                _ => default_at,
            }
        })
        .collect();
    let source = with_default(&parts.values, elem)?;
    take_indexes(&source, &picks)
}

fn map_element(c: &Call<'_>, k: &ChType, v: &ChType) -> Result<ArrayRef, ChError> {
    let parts = ListParts::of(&c.args[0])?;
    let entries = parts.values.as_struct();
    let keys = Arc::clone(entries.column(0));
    let values = Arc::clone(entries.column(1));
    let (keys, needles) = comparable(&keys, k, &c.args[1], &c.types[1])?;
    let rows = eq_keys(&[keys, needles])?;
    let default_at = values.len() as u64;
    let picks: Vec<u64> = (0..c.rows)
        .map(|r| {
            let needle = rows[1].row(r);
            parts
                .range(r)
                .find(|j| rows[0].row(*j) == needle)
                .map_or(default_at, |j| j as u64)
        })
        .collect();
    let source = with_default(&values, v)?;
    take_indexes(&source, &picks)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use datafusion::arrow::datatypes::UInt8Type;
    use loams_house_types::arrow::data_type;

    use super::*;
    use crate::func::nested::values::build_list;

    fn arrays() -> (ArrayRef, ChType) {
        let t = ChType::Array(Box::new(ChType::UInt8));
        let list = build_list(
            &data_type(&t).unwrap().0,
            [3, 0],
            Arc::new(UInt8Array::from(vec![10, 20, 30])),
        )
        .unwrap();
        (list, t)
    }

    #[test]
    fn elements_count_from_one_and_from_the_end() {
        let (list, t) = arrays();
        for (i, want) in [(1i64, [10, 0]), (-1, [30, 0]), (4, [0, 0]), (0, [0, 0])] {
            let idx: ArrayRef = Arc::new(datafusion::arrow::array::Int64Array::from(vec![i, i]));
            let c = Call {
                name: "arrayElement",
                args: &[Arc::clone(&list), idx],
                consts: &[None, None],
                types: &[t.clone(), ChType::Int64],
                rows: 2,
                ret: &ChType::UInt8,
            };
            let out = array_element(&c, &ChType::UInt8).unwrap();
            let out = out.as_primitive::<UInt8Type>();
            assert_eq!([out.value(0), out.value(1)], want, "index {i}");
        }
    }

    #[test]
    fn index_of_and_has_find_the_first_match() {
        let (list, t) = arrays();
        let needle: ArrayRef = Arc::new(UInt8Array::from(vec![20, 20]));
        let c = Call {
            name: "indexOf",
            args: &[list, needle],
            consts: &[None, None],
            types: &[t, ChType::UInt8],
            rows: 2,
            ret: &ChType::UInt64,
        };
        assert_eq!(first_match(&c).unwrap(), vec![Some(1), None]);
    }
}
