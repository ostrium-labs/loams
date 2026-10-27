//! ClickHouse's map functions on the Rust engine (plan HE1 Task 32): `map`
//! (and `{k: v}`), `mapKeys`, `mapValues`, `mapContains` (canonically
//! `mapContainsKey`) and `mapFromArrays` (ClickHouse `src/Functions/map.cpp`,
//! Apache-2.0). `m[k]` is `arrayElement` ([`crate::func::arrays::access`]),
//! and `mapFilter` takes a lambda ([`crate::func::lambda`]).
//!
//! `map` registers as `__ch_map`, since DataFusion's planner treats `map`
//! itself specially (R22.3).

use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, MapArray, StructArray, UInt8Array};
use datafusion::arrow::compute::interleave;
use datafusion::arrow::datatypes::DataType;
use loams_house_sql::Registry;
use loams_house_types::arrow::data_type;
use loams_house_types::{ChError, ChType};

use crate::func::nested::rows::{comparable, comparison_type, eq_keys};
use crate::func::nested::supertype::supertype;
use crate::func::nested::values::{ListParts, build_list, offsets_from, take_indexes, to_type};
use crate::func::nested::{
    Composite, arity, arrow_err, check_arity, coerce_to, element_of, illegal_type, sizes_dont_match,
};

/// The canonical names of the functions this module registers.
pub const NAMES: &[&str] = &[
    "map",
    "mapKeys",
    "mapValues",
    "mapContainsKey",
    "mapFromArrays",
];

/// A map column of type `map_type` from row lengths and its keys and values
/// end to end.
pub fn build_map(
    map_type: &DataType,
    lengths: &[usize],
    keys: ArrayRef,
    values: ArrayRef,
) -> Result<ArrayRef, ChError> {
    let DataType::Map(entries_field, sorted) = map_type else {
        return Err(ChError::logical_error("A Map is not an Arrow map"));
    };
    let DataType::Struct(fields) = entries_field.data_type() else {
        return Err(ChError::logical_error("Map entries are not a Struct"));
    };
    let keys = coerce_to(&keys, fields[0].data_type())?;
    let values = coerce_to(&values, fields[1].data_type())?;
    let entries =
        StructArray::try_new(fields.clone(), vec![keys, values], None).map_err(arrow_err)?;
    Ok(Arc::new(
        MapArray::try_new(
            Arc::clone(entries_field),
            offsets_from(lengths),
            entries,
            None,
            *sorted,
        )
        .map_err(arrow_err)?,
    ))
}

fn map_of<'t>(name: &str, types: &'t [ChType]) -> Result<(&'t ChType, &'t ChType), ChError> {
    match types.first() {
        Some(ChType::Map(k, v)) => Ok((k, v)),
        Some(t) => Err(illegal_type(name, 0, t)),
        None => Err(arity(name, 0, "1")),
    }
}

/// Registers the map functions.
pub fn register(r: &mut Registry) {
    // `map(k1, v1, k2, v2, …)`.
    Composite::new(
        "map",
        Arc::new(|types, _| {
            if types.len() % 2 != 0 {
                return Err(arity("map", types.len(), "an even number"));
            }
            let keys: Vec<ChType> = types.iter().step_by(2).cloned().collect();
            let values: Vec<ChType> = types.iter().skip(1).step_by(2).cloned().collect();
            Ok(ChType::Map(
                Box::new(supertype(&keys)?),
                Box::new(supertype(&values)?),
            ))
        }),
        Arc::new(|c| {
            let ChType::Map(kt, vt) = c.ret else {
                return Err(ChError::logical_error("map is not typed Map"));
            };
            let map_type = data_type(c.ret)?.0;
            let pairs = c.args.len() / 2;
            let mut keys = Vec::with_capacity(pairs);
            let mut values = Vec::with_capacity(pairs);
            for p in 0..pairs {
                keys.push(to_type(&c.args[2 * p], kt)?);
                values.push(to_type(&c.args[2 * p + 1], vt)?);
            }
            let order: Vec<(usize, usize)> = (0..c.rows)
                .flat_map(|r| (0..pairs).map(move |i| (i, r)))
                .collect();
            let gather = |cols: &[ArrayRef], t: &ChType| -> Result<ArrayRef, ChError> {
                if cols.is_empty() {
                    return Ok(datafusion::arrow::array::new_empty_array(&data_type(t)?.0));
                }
                let refs: Vec<&dyn Array> = cols.iter().map(|a| a.as_ref()).collect();
                interleave(&refs, &order).map_err(arrow_err)
            };
            let keys = gather(&keys, kt)?;
            let values = gather(&values, vt)?;
            build_map(&map_type, &vec![pairs; c.rows], keys, values)
        }),
    )
    .own_nulls()
    .register(r);

    for (name, column) in [("mapKeys", 0usize), ("mapValues", 1usize)] {
        Composite::new(
            name,
            Arc::new(move |types, _| {
                check_arity(name, types.len(), 1, 1)?;
                let (k, v) = map_of(name, types)?;
                Ok(ChType::Array(Box::new(if column == 0 {
                    k.clone()
                } else {
                    v.clone()
                })))
            }),
            Arc::new(move |c| {
                let parts = ListParts::of(&c.args[0])?;
                let entries = parts.values.as_struct();
                let lengths: Vec<usize> = (0..c.rows).map(|r| parts.len(r)).collect();
                let used = entries
                    .column(column)
                    .slice(parts.offsets[0], parts.offsets[c.rows] - parts.offsets[0]);
                build_list(&data_type(c.ret)?.0, lengths, used)
            }),
        )
        .register(r);
    }

    // `mapContains(m, k)` (canonically `mapContainsKey`).
    Composite::new(
        "mapContainsKey",
        Arc::new(|types, _| {
            check_arity("mapContainsKey", types.len(), 2, 2)?;
            let (k, _) = map_of("mapContainsKey", types)?;
            comparison_type(k, &types[1])?;
            Ok(ChType::UInt8)
        }),
        Arc::new(|c| {
            let (kt, _) = map_of(c.name, c.types)?;
            let parts = ListParts::of(&c.args[0])?;
            let keys = Arc::clone(parts.values.as_struct().column(0));
            let (keys, needles) = comparable(&keys, kt, &c.args[1], &c.types[1])?;
            let k = eq_keys(&[keys, needles])?;
            Ok(Arc::new(UInt8Array::from(
                (0..c.rows)
                    .map(|r| u8::from(parts.range(r).any(|j| k[0].row(j) == k[1].row(r))))
                    .collect::<Vec<_>>(),
            )) as ArrayRef)
        }),
    )
    .own_nulls()
    .register(r);

    // `mapFromArrays(keys, values)`: the arrays must be the same length.
    Composite::new(
        "mapFromArrays",
        Arc::new(|types, _| {
            check_arity("mapFromArrays", types.len(), 2, 2)?;
            let k = element_of("mapFromArrays", types, 0)?;
            let v = element_of("mapFromArrays", types, 1)?;
            Ok(ChType::Map(Box::new(k.clone()), Box::new(v.clone())))
        }),
        Arc::new(|c| {
            let keys = ListParts::of(&c.args[0])?;
            let values = ListParts::of(&c.args[1])?;
            let mut lengths = Vec::with_capacity(c.rows);
            let (mut ki, mut vi) = (Vec::new(), Vec::new());
            for r in 0..c.rows {
                if keys.len(r) != values.len(r) {
                    return Err(sizes_dont_match("mapFromArrays"));
                }
                lengths.push(keys.len(r));
                ki.extend(keys.range(r).map(|j| j as u64));
                vi.extend(values.range(r).map(|j| j as u64));
            }
            build_map(
                &data_type(c.ret)?.0,
                &lengths,
                take_indexes(&keys.values, &ki)?,
                take_indexes(&values.values, &vi)?,
            )
        }),
    )
    .register(r);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use datafusion::arrow::array::{StringArray, UInt8Array};

    use super::*;

    #[test]
    fn maps_build_from_keys_and_values() {
        let t = ChType::Map(Box::new(ChType::String), Box::new(ChType::UInt8));
        let m = build_map(
            &data_type(&t).unwrap().0,
            &[2, 0],
            Arc::new(StringArray::from(vec!["a", "b"])),
            Arc::new(UInt8Array::from(vec![1, 2])),
        )
        .unwrap();
        assert_eq!(m.len(), 2);
        let parts = ListParts::of(&m).unwrap();
        assert_eq!(parts.offsets, vec![0, 2, 2]);
    }
}
