//! ClickHouse's tuple functions on the Rust engine (plan HE1 Task 32):
//! `tuple` (and `(a, b)`), `tupleElement` (and `t.1`), and `untuple`
//! (ClickHouse `src/Functions/tuple.cpp`, `tupleElement.cpp`, Apache-2.0).
//!
//! `untuple(t)` as a select item becomes one column per element, named
//! `tupleElement(t, '<name>')`, after `SqlToRel` (`loams_house_sql::untuple`);
//! the function registered here only types it, and refuses with `48` where
//! it was not a select item.

use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, AsArray, StructArray};
use datafusion::arrow::datatypes::DataType;
use datafusion::common::ScalarValue;
use loams_house_sql::Registry;
use loams_house_types::arrow::data_type;
use loams_house_types::{ChError, ChType};

use crate::func::nested::values::{ListParts, build_list, to_type};
use crate::func::nested::{Composite, arrow_err, check_arity, coerce_to, illegal_type};

/// The canonical names of the functions this module registers.
pub const NAMES: &[&str] = &["tuple", "tupleElement", "untuple"];

/// `127 ILLEGAL_INDEX`.
fn illegal_index(message: String) -> ChError {
    ChError {
        code: 127,
        name: "ILLEGAL_INDEX",
        message,
    }
}

/// `10 NOT_FOUND_COLUMN_IN_BLOCK`.
fn no_such_element(name: &str) -> ChError {
    ChError {
        code: 10,
        name: "NOT_FOUND_COLUMN_IN_BLOCK",
        message: format!("Tuple doesn't have element with name '{name}'"),
    }
}

/// Which element `tupleElement`'s second argument names, from its constant
/// value, in a tuple of `elements`: its position from 0, or `None` when the
/// tuple has no such element.
pub fn element_position(
    elements: &[(Option<String>, ChType)],
    which: &ScalarValue,
) -> Result<Option<usize>, ChError> {
    if let Some(i) = crate::func::nested::scalar_i64(which) {
        if i >= 1 && (i as usize) <= elements.len() {
            return Ok(Some(i as usize - 1));
        }
        return Ok(None);
    }
    let name = match which {
        ScalarValue::Utf8(Some(s))
        | ScalarValue::LargeUtf8(Some(s))
        | ScalarValue::Utf8View(Some(s)) => s.clone(),
        other => {
            return Err(ChError::illegal_type_of_argument(format!(
                "Second argument to tupleElement must be a constant UInt or String, got {other}"
            )));
        }
    };
    Ok(elements.iter().enumerate().find_map(|(i, (n, _))| {
        let matches = match n {
            Some(n) => *n == name,
            None => (i + 1).to_string() == name,
        };
        matches.then_some(i)
    }))
}

/// The type `tupleElement` answers for argument types `types`, with the
/// constant `which`.
fn element_type(types: &[ChType], which: Option<&ScalarValue>) -> Result<ChType, ChError> {
    let which = which.ok_or_else(|| {
        ChError::illegal_type_of_argument(
            "Second argument to tupleElement must be a constant UInt or String",
        )
    })?;
    match &types[0] {
        ChType::Array(inner) => {
            let mut nested = types.to_vec();
            nested[0] = (**inner).clone();
            Ok(ChType::Array(Box::new(element_type(&nested, Some(which))?)))
        }
        ChType::Tuple(elements) => match element_position(elements, which)? {
            Some(i) => Ok(elements[i].1.clone()),
            None => match types.get(2) {
                Some(default) => Ok(default.clone()),
                None => Err(match crate::func::nested::scalar_i64(which) {
                    Some(i) => illegal_index(format!(
                        "Index for tuple element is out of range: {i}, tuple has {} elements",
                        elements.len()
                    )),
                    None => no_such_element(&which.to_string()),
                }),
            },
        },
        other => Err(illegal_type("tupleElement", 0, other)),
    }
}

/// Element `which` of the tuples in `a` (of type `t`), or of the tuples
/// inside its arrays.
fn element_of_column(
    a: &ArrayRef,
    t: &ChType,
    which: &ScalarValue,
    default: Option<&ArrayRef>,
    ret: &ChType,
) -> Result<ArrayRef, ChError> {
    match t {
        ChType::Array(inner) => {
            let ChType::Array(ret_inner) = ret else {
                return Err(ChError::logical_error(
                    "tupleElement of an array is not an array",
                ));
            };
            let parts = ListParts::of(a)?;
            let rows = parts.rows();
            let values = element_of_column(&parts.values, inner, which, None, ret_inner)?;
            let used = values.slice(parts.offsets[0], parts.offsets[rows] - parts.offsets[0]);
            build_list(&data_type(ret)?.0, (0..rows).map(|r| parts.len(r)), used)
        }
        ChType::Tuple(elements) => match element_position(elements, which)? {
            Some(i) => Ok(Arc::clone(a.as_struct().column(i))),
            None => match default {
                Some(d) => to_type(d, ret),
                None => Err(no_such_element(&which.to_string())),
            },
        },
        other => Err(illegal_type("tupleElement", 0, other)),
    }
}

/// Registers the tuple functions.
pub fn register(r: &mut Registry) {
    // `tuple(a, b, …)`: an unnamed tuple of the arguments' types.
    Composite::new(
        "tuple",
        Arc::new(|types, _| {
            Ok(ChType::Tuple(
                types.iter().map(|t| (None, t.clone())).collect(),
            ))
        }),
        Arc::new(|c| {
            let dt = data_type(c.ret)?.0;
            let DataType::Struct(fields) = &dt else {
                return Err(ChError::logical_error("tuple is not typed Tuple"));
            };
            if fields.is_empty() {
                return Ok(Arc::new(StructArray::new_empty_fields(c.rows, None)));
            }
            let columns = c
                .args
                .iter()
                .zip(fields.iter())
                .map(|(a, f)| coerce_to(a, f.data_type()))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(
                Arc::new(StructArray::try_new(fields.clone(), columns, None).map_err(arrow_err)?)
                    as ArrayRef,
            )
        }),
    )
    .own_nulls()
    .register(r);

    // `tupleElement(t, i | 'name'[, default])`.
    Composite::new(
        "tupleElement",
        Arc::new(|types, scalars| {
            check_arity("tupleElement", types.len(), 2, 3)?;
            element_type(types, scalars.get(1).copied().flatten())
        }),
        Arc::new(|c| {
            let which = c.consts.get(1).and_then(Option::as_ref).ok_or_else(|| {
                ChError::illegal_type_of_argument(
                    "Second argument to tupleElement must be a constant UInt or String",
                )
            })?;
            element_of_column(&c.args[0], &c.types[0], which, c.args.get(2), c.ret)
        }),
    )
    .own_nulls()
    .register(r);

    // `untuple(t)`: typed as `t`; expanded where it is a select item.
    Composite::new(
        "untuple",
        Arc::new(|types, _| {
            check_arity("untuple", types.len(), 1, 1)?;
            match &types[0] {
                t @ ChType::Tuple(_) => Ok(t.clone()),
                other => Err(illegal_type("untuple", 0, other)),
            }
        }),
        Arc::new(|_| {
            Err(ChError::unsupported(
                "untuple anywhere but as a select item",
            ))
        }),
    )
    .own_nulls()
    .register(r);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn elements_are_found_by_index_and_name() {
        let named = vec![
            (Some("a".to_string()), ChType::UInt8),
            (Some("b".to_string()), ChType::String),
        ];
        let unnamed = vec![(None, ChType::UInt8), (None, ChType::String)];
        let two = ScalarValue::UInt8(Some(2));
        let b = ScalarValue::Utf8(Some("b".into()));
        let one = ScalarValue::Utf8(Some("1".into()));
        assert_eq!(element_position(&named, &two).unwrap(), Some(1));
        assert_eq!(element_position(&named, &b).unwrap(), Some(1));
        assert_eq!(element_position(&unnamed, &one).unwrap(), Some(0));
        assert_eq!(
            element_position(&unnamed, &ScalarValue::Int64(Some(3))).unwrap(),
            None
        );
        let t = ChType::Tuple(named);
        assert_eq!(
            element_type(
                &[t.clone(), ChType::UInt8],
                Some(&ScalarValue::UInt8(Some(9)))
            )
            .unwrap_err()
            .code,
            127
        );
        assert_eq!(
            element_type(&[ChType::Array(Box::new(t)), ChType::String], Some(&b)).unwrap(),
            ChType::Array(Box::new(ChType::String))
        );
    }
}
