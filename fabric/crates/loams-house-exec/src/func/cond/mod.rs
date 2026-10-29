//! Conditionals (plan HE1 Task 33): `if`, `multiIf`, `caseWithExpression`
//! (`CASE x WHEN …`), `ifNull`, `nullIf`, `coalesce`, `greatest`, `least`,
//! `isNull`, `isNotNull`, `isZeroOrNull`, `isNaN`, `isFinite`, `isInfinite`
//! and `ifNotFinite`, from `src/Functions/{if,multiIf,caseWithExpression,
//! ifNull,nullIf,coalesce,greatest,least,isNull,isNotNull,isNaN}.cpp`
//! (Apache-2.0).
//!
//! Results take the arguments' least supertype ([`supertype`]). Every branch
//! is evaluated before the choice is made (R33.7): ClickHouse's short-circuit
//! evaluation (`short_circuit_function_evaluation`) would skip an error in a
//! branch no row takes, which the engine does not yet.

pub mod supertype;

use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, BooleanArray, UInt8Array, new_null_array};
use datafusion::arrow::compute::kernels::cmp::{eq, gt, lt};
use datafusion::arrow::compute::kernels::nullif::nullif;
use datafusion::arrow::compute::kernels::zip::zip;
use datafusion::arrow::datatypes::DataType;
use loams_house_sql::Registry;
use loams_house_types::arrow::{data_type, nullable};
use loams_house_types::{ChError, ChType};

use crate::func::convert::cast::Mode;
use crate::func::convert::cast::cast_column;
use crate::func::convert::not_null;
use crate::func::convert::value::{Value, float_at, read};
use crate::func::typed::{
    ChFn, Nulls, Sig, arity, arrow_error, illegal_type, is_number, kernel, typer,
};
use supertype::least_supertype;

/// `t` without `LowCardinality` at the top (`LowCardinality(Nullable(T))` is
/// `Nullable(T)`): the type of a decoded column.
pub fn plain(t: &ChType) -> ChType {
    match t {
        ChType::LowCardinality(inner) => plain(inner),
        other => other.clone(),
    }
}

/// `array` (a decoded column of `from`) in the Arrow layout of
/// `Nullable(to)`, `to` taken without `LowCardinality`: the layout the
/// conditionals compute in.
pub fn to_work(array: &ArrayRef, from: &ChType, to: &ChType) -> Result<ArrayRef, ChError> {
    let from = plain(from);
    let to = plain(to);
    if from.strip_nullable() == to.strip_nullable() {
        return Ok(Arc::clone(array));
    }
    if matches!(to.strip_nullable(), ChType::Nothing) {
        return Ok(new_null_array(&DataType::Null, array.len()));
    }
    cast_column(array.as_ref(), &from, &nullable(to), &Mode::Throw)
}

/// Rows whose value is true for a condition (`NULL` is false), as
/// ClickHouse's `if` reads a `UInt8`, a `Bool` or any number.
pub fn truthy(array: &dyn Array, t: &ChType) -> Result<BooleanArray, ChError> {
    let mut out = Vec::with_capacity(array.len());
    for row in 0..array.len() {
        out.push(match read(array, t, row)? {
            Value::Null => false,
            Value::Bool(b) => b,
            Value::Int(i) => i != 0,
            Value::Float(f) => f != 0.0,
            Value::Dec(u, _) => u != 0,
            other => {
                return Err(ChError::illegal_type_of_argument(format!(
                    "Illegal value {other:?} of a condition"
                )));
            }
        });
    }
    Ok(BooleanArray::from(out))
}

fn check_condition(sig: &Sig<'_>, i: usize, function: &str) -> Result<(), ChError> {
    let t = sig.types[i].base();
    if is_number(t) || matches!(t, ChType::Nothing) {
        Ok(())
    } else {
        Err(ChError::illegal_type_of_argument(format!(
            "Illegal type {} of argument (condition) of function {function}. Must be UInt8.",
            sig.types[i]
        )))
    }
}

/// Where a row is `NULL`.
pub fn null_mask(array: &dyn Array) -> BooleanArray {
    let n = array.len();
    match array.logical_nulls() {
        Some(nulls) => BooleanArray::from((0..n).map(|i| nulls.is_null(i)).collect::<Vec<_>>()),
        None => BooleanArray::from(vec![false; n]),
    }
}

fn not(mask: &BooleanArray) -> BooleanArray {
    BooleanArray::from(
        (0..mask.len())
            .map(|i| !(mask.is_valid(i) && mask.value(i)))
            .collect::<Vec<_>>(),
    )
}

fn choose(mask: &BooleanArray, yes: &ArrayRef, no: &ArrayRef) -> Result<ArrayRef, ChError> {
    if yes.data_type() == &DataType::Null && no.data_type() == &DataType::Null {
        return Ok(Arc::clone(yes));
    }
    // A `NULL` branch takes the other branch's layout.
    let yes = if yes.data_type() == &DataType::Null {
        new_null_array(no.data_type(), yes.len())
    } else {
        Arc::clone(yes)
    };
    let no = if no.data_type() == &DataType::Null {
        new_null_array(yes.data_type(), no.len())
    } else {
        Arc::clone(no)
    };
    zip(mask, &yes, &no).map_err(arrow_error)
}

/// Rows where `a` (of `ta`) equals `b` (of `tb`), compared in their
/// supertype; `NULL` equals nothing.
pub fn equal_mask(
    a: &ArrayRef,
    ta: &ChType,
    b: &ArrayRef,
    tb: &ChType,
) -> Result<BooleanArray, ChError> {
    let ta0 = plain(ta).strip_nullable().clone();
    let tb0 = plain(tb).strip_nullable().clone();
    if matches!(ta0, ChType::Nothing) || matches!(tb0, ChType::Nothing) {
        return Ok(BooleanArray::from(vec![false; a.len()]));
    }
    let common = least_supertype(&[ta0, tb0])?;
    let a = to_work(a, ta, &common)?;
    let b = to_work(b, tb, &common)?;
    let e = eq(&a, &b).map_err(arrow_error)?;
    Ok(BooleanArray::from(
        (0..e.len())
            .map(|i| e.is_valid(i) && e.value(i))
            .collect::<Vec<_>>(),
    ))
}

fn uint8(values: Vec<u8>) -> ArrayRef {
    Arc::new(UInt8Array::from(values))
}

/// Registers the conditional functions.
pub fn register(r: &mut Registry) {
    ChFn::new(
        "if",
        Nulls::Own,
        typer(|s| {
            arity(s, 3, 3)?;
            check_condition(s, 0, "if")?;
            least_supertype(&s.types[1..])
        }),
        kernel(|c| {
            let mask = truthy(c.args[0].as_ref(), &c.types[0])?;
            let yes = to_work(&c.args[1], &c.types[1], c.ret)?;
            let no = to_work(&c.args[2], &c.types[2], c.ret)?;
            choose(&mask, &yes, &no)
        }),
    )
    .register(r);

    ChFn::new(
        "multiIf",
        Nulls::Own,
        typer(|s| {
            let n = s.types.len();
            if n < 3 || n.is_multiple_of(2) {
                return Err(ChError::number_of_arguments_doesnt_match(format!(
                    "Invalid number of arguments for function {}",
                    s.name
                )));
            }
            let mut values = Vec::with_capacity(n / 2 + 1);
            for (i, t) in s.types[..n - 1].iter().enumerate() {
                if i.is_multiple_of(2) {
                    check_condition(s, i, "multiIf")?;
                } else {
                    values.push(t.clone());
                }
            }
            values.push(s.types[n - 1].clone());
            least_supertype(&values)
        }),
        kernel(|c| {
            let n = c.args.len();
            let mut out = to_work(&c.args[n - 1], &c.types[n - 1], c.ret)?;
            for i in (0..n - 1).step_by(2).rev() {
                let mask = truthy(c.args[i].as_ref(), &c.types[i])?;
                let value = to_work(&c.args[i + 1], &c.types[i + 1], c.ret)?;
                out = choose(&mask, &value, &out)?;
            }
            Ok(out)
        }),
    )
    .register(r);

    // `CASE x WHEN a THEN b … ELSE e END`; the guard always passes the ELSE
    // (`NULL` when absent).
    ChFn::new(
        "caseWithExpression",
        Nulls::Own,
        typer(|s| {
            let n = s.types.len();
            if n < 4 || !n.is_multiple_of(2) {
                return Err(ChError::number_of_arguments_doesnt_match(format!(
                    "Invalid number of arguments for function {}",
                    s.name
                )));
            }
            let mut values = Vec::with_capacity(n / 2);
            for i in (1..n - 1).step_by(2) {
                let x = plain(&s.types[0]).strip_nullable().clone();
                let w = plain(&s.types[i]).strip_nullable().clone();
                if !matches!(x, ChType::Nothing) && !matches!(w, ChType::Nothing) {
                    least_supertype(&[x, w])?;
                }
                values.push(s.types[i + 1].clone());
            }
            values.push(s.types[n - 1].clone());
            least_supertype(&values)
        }),
        kernel(|c| {
            let n = c.args.len();
            let mut out = to_work(&c.args[n - 1], &c.types[n - 1], c.ret)?;
            for i in (1..n - 1).step_by(2).rev() {
                let mask = equal_mask(&c.args[0], &c.types[0], &c.args[i], &c.types[i])?;
                let value = to_work(&c.args[i + 1], &c.types[i + 1], c.ret)?;
                out = choose(&mask, &value, &out)?;
            }
            Ok(out)
        }),
    )
    .register(r);

    ChFn::new(
        "ifNull",
        Nulls::Own,
        typer(|s| {
            arity(s, 2, 2)?;
            if !s.types[0].is_nullable() {
                return Ok(s.types[0].clone());
            }
            if matches!(s.types[0].base(), ChType::Nothing) {
                return Ok(s.types[1].clone());
            }
            least_supertype(&[not_null(&s.types[0]), s.types[1].clone()])
        }),
        kernel(|c| {
            if !c.types[0].is_nullable() {
                return Ok(Arc::clone(&c.args[0]));
            }
            let a = to_work(&c.args[0], &c.types[0], c.ret)?;
            let b = to_work(&c.args[1], &c.types[1], c.ret)?;
            let present = not(&null_mask(c.args[0].as_ref()));
            choose(&present, &a, &b)
        }),
    )
    .register(r);

    ChFn::new(
        "nullIf",
        Nulls::Own,
        typer(|s| {
            arity(s, 2, 2)?;
            let x = plain(&s.types[0]).strip_nullable().clone();
            let y = plain(&s.types[1]).strip_nullable().clone();
            if !matches!(x, ChType::Nothing) && !matches!(y, ChType::Nothing) {
                least_supertype(&[x, y])?;
            }
            Ok(nullable(plain(&s.types[0])))
        }),
        kernel(|c| {
            let a = to_work(&c.args[0], &c.types[0], c.ret)?;
            if a.data_type() == &DataType::Null {
                return Ok(a);
            }
            let mask = equal_mask(&c.args[0], &c.types[0], &c.args[1], &c.types[1])?;
            nullif(a.as_ref(), &mask).map_err(arrow_error)
        }),
    )
    .register(r);

    ChFn::new(
        "coalesce",
        Nulls::Own,
        typer(|s| {
            arity(s, 1, usize::MAX)?;
            let kept = coalesce_args(s.types);
            if kept.is_empty() {
                return Ok(ChType::Nullable(Box::new(ChType::Nothing)));
            }
            let bases: Vec<ChType> = kept.iter().map(|i| not_null(&s.types[*i])).collect();
            let base = least_supertype(&bases)?;
            let any_not_null = kept.iter().any(|i| !s.types[*i].is_nullable());
            Ok(if any_not_null { base } else { nullable(base) })
        }),
        kernel(|c| {
            let kept = coalesce_args(c.types);
            let Some((first, rest)) = kept.split_first() else {
                return Ok(new_null_array(&DataType::Null, c.rows));
            };
            let mut out = to_work(&c.args[*first], &c.types[*first], c.ret)?;
            for i in rest {
                let next = to_work(&c.args[*i], &c.types[*i], c.ret)?;
                let missing = null_mask(out.as_ref());
                out = choose(&missing, &next, &out)?;
            }
            Ok(out)
        }),
    )
    .register(r);

    for (name, greater) in [("greatest", true), ("least", false)] {
        ChFn::new(
            name,
            Nulls::Own,
            typer(|s| {
                arity(s, 2, usize::MAX)?;
                least_supertype(s.types)
            }),
            kernel(move |c| {
                let mut out = to_work(&c.args[0], &c.types[0], c.ret)?;
                for (arg, t) in c.args.iter().zip(c.types).skip(1) {
                    let next = to_work(arg, t, c.ret)?;
                    if next.data_type() == &DataType::Null {
                        continue;
                    }
                    if out.data_type() == &DataType::Null {
                        out = next;
                        continue;
                    }
                    let better = if greater {
                        gt(&next, &out)
                    } else {
                        lt(&next, &out)
                    }
                    .map_err(arrow_error)?;
                    // `NULL`s are skipped (ClickHouse 24.12 and later).
                    let take: Vec<bool> = (0..out.len())
                        .map(|r| {
                            next.is_valid(r)
                                && (out.is_null(r) || (better.is_valid(r) && better.value(r)))
                        })
                        .collect();
                    out = choose(&BooleanArray::from(take), &next, &out)?;
                }
                Ok(out)
            }),
        )
        .register(r);
    }

    for (name, want_null) in [("isNull", true), ("isNotNull", false)] {
        ChFn::new(
            name,
            Nulls::Own,
            typer(|s| {
                arity(s, 1, 1)?;
                Ok(ChType::UInt8)
            }),
            kernel(move |c| {
                let mask = null_mask(c.args[0].as_ref());
                Ok(uint8(
                    (0..c.rows)
                        .map(|i| u8::from(mask.value(i) == want_null))
                        .collect(),
                ))
            }),
        )
        .plain()
        .register(r);
    }

    ChFn::new(
        "isZeroOrNull",
        Nulls::Own,
        typer(|s| {
            arity(s, 1, 1)?;
            if !is_number(s.types[0].base()) && !matches!(s.types[0].base(), ChType::Nothing) {
                return Err(illegal_type(s.name, 0, &s.types[0]));
            }
            Ok(ChType::UInt8)
        }),
        kernel(|c| {
            let mut out = Vec::with_capacity(c.rows);
            for row in 0..c.rows {
                out.push(u8::from(
                    match read(c.args[0].as_ref(), &c.types[0], row)? {
                        Value::Null => true,
                        Value::Int(i) => i == 0,
                        Value::Float(f) => f == 0.0,
                        Value::Bool(b) => !b,
                        Value::Dec(u, _) => u == 0,
                        _ => false,
                    },
                ));
            }
            Ok(uint8(out))
        }),
    )
    .plain()
    .register(r);

    type FloatTest = fn(f64) -> bool;
    let tests: [(&str, FloatTest); 3] = [
        ("isNaN", f64::is_nan),
        ("isFinite", f64::is_finite),
        ("isInfinite", f64::is_infinite),
    ];
    for (name, test) in tests {
        ChFn::new(
            name,
            Nulls::Default,
            typer(|s| {
                arity(s, 1, 1)?;
                if !is_number(&s.types[0]) {
                    return Err(illegal_type(s.name, 0, &s.types[0]));
                }
                Ok(ChType::UInt8)
            }),
            kernel(move |c| {
                let a = c.args[0].as_ref();
                let floats = matches!(c.types[0], ChType::Float32 | ChType::Float64);
                Ok(uint8(
                    (0..c.rows)
                        .map(|i| {
                            let f = if floats && a.is_valid(i) {
                                float_at(a, i).unwrap_or(0.0)
                            } else {
                                0.0
                            };
                            u8::from(test(f))
                        })
                        .collect(),
                ))
            }),
        )
        .plain()
        .register(r);
    }

    ChFn::new(
        "ifNotFinite",
        Nulls::Own,
        typer(|s| {
            arity(s, 2, 2)?;
            least_supertype(s.types)
        }),
        kernel(|c| {
            let x = c.args[0].as_ref();
            let floats = matches!(
                plain(&c.types[0]).strip_nullable(),
                ChType::Float32 | ChType::Float64
            );
            let finite: Vec<bool> = (0..c.rows)
                .map(|i| !floats || x.is_null(i) || float_at(x, i).is_some_and(f64::is_finite))
                .collect();
            let a = to_work(&c.args[0], &c.types[0], c.ret)?;
            let b = to_work(&c.args[1], &c.types[1], c.ret)?;
            choose(&BooleanArray::from(finite), &a, &b)
        }),
    )
    .register(r);
}

/// The arguments `coalesce` reads: not `NULL` literals, and none after the
/// first that cannot be `NULL`.
fn coalesce_args(types: &[ChType]) -> Vec<usize> {
    let mut kept = Vec::new();
    for (i, t) in types.iter().enumerate() {
        if matches!(t.base(), ChType::Nothing) {
            continue;
        }
        kept.push(i);
        if !t.is_nullable() {
            break;
        }
    }
    kept
}

/// The Arrow type of `t`, for tests.
pub fn arrow_type(t: &ChType) -> Result<DataType, ChError> {
    Ok(data_type(t)?.0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use datafusion::arrow::array::{AsArray, Int64Array, StringArray};
    use datafusion::arrow::datatypes::Int16Type;

    #[test]
    fn conditions_read_numbers() {
        let a: ArrayRef = Arc::new(Int64Array::from(vec![Some(0), Some(2), None]));
        let m = truthy(a.as_ref(), &ChType::Nullable(Box::new(ChType::Int64))).unwrap();
        assert_eq!((m.value(0), m.value(1), m.value(2)), (false, true, false));
    }

    #[test]
    fn branches_take_the_supertype() {
        let x: ArrayRef = Arc::new(Int64Array::from(vec![1, 2]));
        let w = to_work(&x, &ChType::Int64, &ChType::Int16).unwrap();
        assert_eq!(w.as_primitive::<Int16Type>().value(1), 2);
        let s: ArrayRef = Arc::new(StringArray::from(vec!["a", "b"]));
        let m = equal_mask(&s, &ChType::String, &s, &ChType::String).unwrap();
        assert!(m.value(0) && m.value(1));
        assert_eq!(arrow_type(&ChType::UInt8).unwrap(), DataType::UInt8);
    }

    #[test]
    fn coalesce_stops_at_the_first_non_nullable() {
        let n = ChType::Nullable(Box::new(ChType::UInt8));
        assert_eq!(
            coalesce_args(&[
                ChType::Nullable(Box::new(ChType::Nothing)),
                n.clone(),
                ChType::UInt8,
                n
            ]),
            vec![1, 2]
        );
    }
}
