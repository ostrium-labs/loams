//! Kernels keep integer comparisons exact and wrap only at the result width.
use super::{
    Op,
    types::{base, error, number},
};
use datafusion::{
    arrow::{array::ListArray, buffer::OffsetBuffer, datatypes::i256},
    common::ScalarValue,
};
use loams_house_types::{ChError, ChType};
use std::{cmp::Ordering, sync::Arc};

fn int(v: &ScalarValue) -> Option<i128> {
    match v {
        ScalarValue::UInt8(Some(v)) => Some(*v as i128),
        ScalarValue::UInt16(Some(v)) => Some(*v as i128),
        ScalarValue::UInt32(Some(v)) => Some(*v as i128),
        ScalarValue::UInt64(Some(v)) => Some(*v as i128),
        ScalarValue::Int8(Some(v)) => Some(*v as i128),
        ScalarValue::Int16(Some(v)) => Some(*v as i128),
        ScalarValue::Int32(Some(v)) => Some(*v as i128),
        ScalarValue::Int64(Some(v)) => Some(*v as i128),
        ScalarValue::Boolean(Some(v)) => Some(i128::from(*v)),
        _ => None,
    }
}
fn float(v: &ScalarValue) -> Option<f64> {
    match v {
        ScalarValue::Float32(Some(v)) => Some(*v as f64),
        ScalarValue::Float64(Some(v)) => Some(*v),
        _ => int(v).map(|v| v as f64),
    }
}
fn truth(v: &ScalarValue) -> Result<Option<bool>, ChError> {
    if v.is_null() {
        return Ok(None);
    }
    float(v)
        .map(|n| Some(n != 0.0))
        .ok_or_else(|| ChError::illegal_type_of_argument("Boolean contexts require numbers"))
}
fn null(t: &ChType) -> Result<ScalarValue, ChError> {
    ScalarValue::try_new_null(&loams_house_types::arrow::data_type(t)?.0)
        .map_err(|e| ChError::logical_error(e.to_string()))
}
pub(super) fn result_int(value: i128, t: &ChType) -> Result<ScalarValue, ChError> {
    use ChType::*;
    Ok(match base(t) {
        Bool => ScalarValue::Boolean(Some(value != 0)),
        UInt8 => ScalarValue::UInt8(Some(value as u8)),
        UInt16 => ScalarValue::UInt16(Some(value as u16)),
        UInt32 => ScalarValue::UInt32(Some(value as u32)),
        UInt64 => ScalarValue::UInt64(Some(value as u64)),
        Int8 | Enum8(_) => ScalarValue::Int8(Some(value as i8)),
        Int16 | Enum16(_) => ScalarValue::Int16(Some(value as i16)),
        Int32 => ScalarValue::Int32(Some(value as i32)),
        Int64 => ScalarValue::Int64(Some(value as i64)),
        _ => return Err(ChError::illegal_type_of_argument("Expected integer result")),
    })
}
fn decimal(v: &ScalarValue) -> Option<(i256, u8)> {
    match v {
        ScalarValue::Decimal128(Some(n), _, s) => Some((i256::from_i128(*n), *s as u8)),
        ScalarValue::Decimal256(Some(n), _, s) => Some((*n, *s as u8)),
        _ => int(v).map(|n| (i256::from_i128(n), 0)),
    }
}
fn overflow() -> ChError {
    error(407, "DECIMAL_OVERFLOW", "Decimal overflow")
}
fn scale(n: i256, from: u8, to: u8) -> Result<i256, ChError> {
    if to >= from {
        n.checked_mul(
            i256::from_i128(10)
                .checked_pow(u32::from(to - from))
                .ok_or_else(overflow)?,
        )
        .ok_or_else(overflow)
    } else {
        n.checked_div(
            i256::from_i128(10)
                .checked_pow(u32::from(from - to))
                .ok_or_else(overflow)?,
        )
        .ok_or_else(overflow)
    }
}
fn decimal_op(op: Op, args: &[ScalarValue], p: u8, s: u8) -> Result<ScalarValue, ChError> {
    let (a, sa) =
        decimal(&args[0]).ok_or_else(|| ChError::illegal_type_of_argument("Decimal operand"))?;
    let (b, sb) = if args.len() > 1 {
        decimal(&args[1]).ok_or_else(|| ChError::illegal_type_of_argument("Decimal operand"))?
    } else {
        (i256::ZERO, 0)
    };
    let value = match op {
        Op::Add => scale(a, sa, s)?.checked_add(scale(b, sb, s)?),
        Op::Subtract => scale(a, sa, s)?.checked_sub(scale(b, sb, s)?),
        Op::Multiply => a
            .checked_mul(b)
            .map(|v| (v, sa + sb))
            .map(|(v, from)| scale(v, from, s))
            .transpose()?,
        Op::Divide => scale(a, sa, s + sb)?.checked_div(b),
        Op::Modulo => a.checked_rem(b),
        Op::Negate => a.checked_neg(),
        _ => None,
    }
    .ok_or_else(|| {
        if matches!(op, Op::Divide | Op::Modulo) && b == i256::ZERO {
            error(153, "ILLEGAL_DIVISION", "Division by zero")
        } else {
            overflow()
        }
    })?;
    // Decimal32/64 check their storage width; wider ClickHouse decimals wrap.
    if p <= 9 && value.to_i128().is_none_or(|v| i32::try_from(v).is_err())
        || p <= 18 && value.to_i128().is_none_or(|v| i64::try_from(v).is_err())
    {
        return Err(overflow());
    }
    if p <= 38 {
        Ok(ScalarValue::Decimal128(
            Some(value.to_i128().ok_or_else(overflow)?),
            p,
            s as i8,
        ))
    } else {
        Ok(ScalarValue::Decimal256(Some(value), p, s as i8))
    }
}
fn int_float_cmp(n: i128, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f >= i128::MAX as f64 {
        return Some(Ordering::Less);
    }
    if f < i128::MIN as f64 {
        return Some(Ordering::Greater);
    }
    let floor = f.floor();
    match n.cmp(&(floor as i128)) {
        Ordering::Equal if f > floor => Some(Ordering::Less),
        other => Some(other),
    }
}
fn compare(
    a: &ScalarValue,
    b: &ScalarValue,
    ta: &ChType,
    tb: &ChType,
) -> Result<Option<Ordering>, ChError> {
    if let (Some(a), Some(b)) = (int(a), int(b)) {
        return Ok(Some(a.cmp(&b)));
    }
    if let (Some(a), Some(b)) = (decimal(a), decimal(b)) {
        let s = a.1.max(b.1);
        return Ok(Some(scale(a.0, a.1, s)?.cmp(&scale(b.0, b.1, s)?)));
    }
    if let (Some(a), Some(b)) = (int(a), float(b)) {
        return Ok(int_float_cmp(a, b));
    }
    if let (Some(a), Some(b)) = (float(a), int(b)) {
        return Ok(int_float_cmp(b, a).map(Ordering::reverse));
    }
    if let (Some(a), Some(b)) = (float(a), float(b)) {
        return Ok(a.partial_cmp(&b));
    }
    if let (ScalarValue::Utf8(Some(a)), ScalarValue::Utf8(Some(b))) = (a, b) {
        return Ok(Some(a.cmp(b)));
    }
    // A literal string is converted to the typed operand, as ClickHouse compares it.
    if matches!(a, ScalarValue::Utf8(Some(_))) && !matches!(base(tb), ChType::String) {
        let converted = convert_text(a, tb)?;
        return compare(&converted, b, tb, tb);
    }
    if matches!(b, ScalarValue::Utf8(Some(_))) && !matches!(base(ta), ChType::String) {
        let converted = convert_text(b, ta)?;
        return compare(a, &converted, ta, ta);
    }
    match (a, b) {
        (ScalarValue::Date32(Some(a)), ScalarValue::Date32(Some(b))) => Ok(Some(a.cmp(b))),
        (ScalarValue::Date32(Some(a)), ScalarValue::TimestampSecond(Some(b), _)) => {
            Ok(Some((i64::from(*a) * 86400).cmp(b)))
        }
        (ScalarValue::TimestampSecond(Some(a), _), ScalarValue::Date32(Some(b))) => {
            Ok(Some(a.cmp(&(i64::from(*b) * 86400))))
        }
        (ScalarValue::Struct(a), ScalarValue::Struct(b)) => {
            if a.num_columns() != b.num_columns() {
                return Err(ChError::illegal_type_of_argument("Tuple sizes differ"));
            }
            for i in 0..a.num_columns() {
                let av = ScalarValue::try_from_array(a.column(i).as_ref(), 0)
                    .map_err(|e| ChError::logical_error(e.to_string()))?;
                let bv = ScalarValue::try_from_array(b.column(i).as_ref(), 0)
                    .map_err(|e| ChError::logical_error(e.to_string()))?;
                let at = loams_house_types::arrow::from_arrow(&a.fields()[i])?;
                let bt = loams_house_types::arrow::from_arrow(&b.fields()[i])?;
                let cmp = compare(&av, &bv, &at, &bt)?;
                if cmp != Some(Ordering::Equal) {
                    return Ok(cmp);
                }
            }
            Ok(Some(Ordering::Equal))
        }
        _ => a.partial_cmp(b).map(Some).ok_or_else(|| {
            ChError::illegal_type_of_argument(format!("Cannot compare {ta} with {tb}"))
        }),
    }
}
pub(super) fn convert_text(v: &ScalarValue, t: &ChType) -> Result<ScalarValue, ChError> {
    if let ScalarValue::Utf8(Some(s)) = v {
        match base(t) {
            ChType::Enum8(values) => {
                return values
                    .iter()
                    .find(|(name, _)| name == s)
                    .map(|(_, v)| ScalarValue::Int8(Some(*v)))
                    .ok_or_else(|| {
                        error(
                            691,
                            "UNKNOWN_ELEMENT_OF_ENUM",
                            format!("Unknown enum label {s:?}"),
                        )
                    });
            }
            ChType::Enum16(values) => {
                return values
                    .iter()
                    .find(|(name, _)| name == s)
                    .map(|(_, v)| ScalarValue::Int16(Some(*v)))
                    .ok_or_else(|| {
                        error(
                            691,
                            "UNKNOWN_ELEMENT_OF_ENUM",
                            format!("Unknown enum label {s:?}"),
                        )
                    });
            }
            _ => {}
        }
    }
    v.cast_to(&loams_house_types::arrow::data_type(t)?.0)
        .map_err(|e| ChError::cannot_parse_text(e.to_string()))
}
pub(super) fn evaluate(
    op: Op,
    args: &[ScalarValue],
    types: &[ChType],
    result: &ChType,
) -> Result<ScalarValue, ChError> {
    if matches!(op, Op::IsNull | Op::IsNotNull) {
        return Ok(ScalarValue::UInt8(Some(u8::from(
            args[0].is_null() == (op == Op::IsNull),
        ))));
    }
    if matches!(op, Op::And | Op::Or) {
        let values = args.iter().map(truth).collect::<Result<Vec<_>, _>>()?;
        let decisive = op == Op::Or;
        return if values.contains(&Some(decisive)) {
            Ok(ScalarValue::UInt8(Some(u8::from(decisive))))
        } else if values.contains(&None) {
            null(result)
        } else {
            Ok(ScalarValue::UInt8(Some(u8::from(!decisive))))
        };
    }
    if op == Op::Array {
        let ChType::Array(t) = base(result) else {
            unreachable!()
        };
        let field = Arc::new(loams_house_types::arrow::to_arrow("item", t)?);
        let items = args
            .iter()
            .map(|v| {
                v.cast_to(field.data_type())
                    .map_err(|e| ChError::cannot_convert_type(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let values = if items.is_empty() {
            datafusion::arrow::array::new_empty_array(field.data_type())
        } else {
            ScalarValue::iter_to_array(items).map_err(|e| ChError::logical_error(e.to_string()))?
        };
        return Ok(ScalarValue::List(Arc::new(ListArray::new(
            field,
            OffsetBuffer::new(vec![0, values.len() as i32].into()),
            values,
            None,
        ))));
    }
    if args.iter().any(ScalarValue::is_null) {
        return null(result);
    }
    if op == Op::Not {
        return Ok(ScalarValue::UInt8(truth(&args[0])?.map(|v| u8::from(!v))));
    }
    if matches!(op, Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge) {
        let cmp = compare(&args[0], &args[1], &types[0], &types[1])?;
        let yes = match op {
            Op::Eq => cmp == Some(Ordering::Equal),
            Op::Ne => cmp != Some(Ordering::Equal),
            Op::Lt => cmp == Some(Ordering::Less),
            Op::Le => matches!(cmp, Some(Ordering::Less | Ordering::Equal)),
            Op::Gt => cmp == Some(Ordering::Greater),
            Op::Ge => matches!(cmp, Some(Ordering::Greater | Ordering::Equal)),
            _ => unreachable!(),
        };
        return Ok(ScalarValue::UInt8(Some(u8::from(yes))));
    }
    if let ChType::Decimal { precision, scale } = base(result) {
        return decimal_op(op, args, *precision, *scale);
    }
    if matches!(base(result), ChType::Float32 | ChType::Float64) {
        let a =
            float(&args[0]).ok_or_else(|| ChError::illegal_type_of_argument("Number required"))?;
        let b = if args.len() > 1 {
            float(&args[1]).ok_or_else(|| ChError::illegal_type_of_argument("Number required"))?
        } else {
            0.0
        };
        let n = match op {
            Op::Add => a + b,
            Op::Subtract => a - b,
            Op::Multiply => a * b,
            Op::Divide => a / b,
            Op::Modulo => a - (a / b).trunc() * b,
            Op::Negate => -a,
            _ => unreachable!(),
        };
        return Ok(if matches!(base(result), ChType::Float32) {
            ScalarValue::Float32(Some(n as f32))
        } else {
            ScalarValue::Float64(Some(n))
        });
    }
    let a = int(&args[0]);
    let b = args.get(1).and_then(int);
    let n = match op {
        Op::IntDiv if a.is_none() || b.is_none() => {
            let a = float(&args[0])
                .or_else(|| {
                    decimal(&args[0]).map(|(v, s)| {
                        v.to_string().parse::<f64>().unwrap_or(f64::NAN) / 10f64.powi(i32::from(s))
                    })
                })
                .ok_or_else(|| ChError::illegal_type_of_argument("Number required"))?;
            let b = float(&args[1])
                .or_else(|| {
                    decimal(&args[1]).map(|(v, s)| {
                        v.to_string().parse::<f64>().unwrap_or(f64::NAN) / 10f64.powi(i32::from(s))
                    })
                })
                .ok_or_else(|| ChError::illegal_type_of_argument("Number required"))?;
            let (signed, bits, _) = number(result).expect("integer result");
            let intermediate = if types.iter().any(|t| matches!(base(t), ChType::Float64)) {
                a / b
            } else {
                ((a as f32) / (b as f32)) as f64
            };
            let min = if signed {
                -(2f64.powi(i32::from(bits) - 1))
            } else {
                0.0
            };
            let max = if signed {
                2f64.powi(i32::from(bits) - 1) - 1.0
            } else {
                2f64.powi(i32::from(bits)) - 1.0
            };
            if !intermediate.is_finite() || intermediate >= max || intermediate <= min {
                return Err(error(
                    153,
                    "ILLEGAL_DIVISION",
                    "Cannot perform integer division, because it will produce infinite or too large number",
                ));
            }
            intermediate.trunc() as i128
        }
        _ => {
            let mut a = a.ok_or_else(|| ChError::illegal_type_of_argument("Integer required"))?;
            let mut b = b.unwrap_or(0);
            let mut division_min = None;
            if op == Op::IntDiv {
                let (sa, ba, _) = number(&types[0]).expect("integer");
                let (sb, bb, _) = number(&types[1]).expect("integer");
                if sa || sb {
                    a = sign_cast(a, ba);
                    b = sign_cast(b, ba.max(bb));
                    division_min = Some(-(1i128 << (ba - 1)));
                }
            }
            match op {
                Op::Add => a.wrapping_add(b),
                Op::Subtract => a.wrapping_sub(b),
                Op::Multiply => a.wrapping_mul(b),
                Op::Negate => a.wrapping_neg(),
                Op::IntDiv | Op::Modulo => {
                    let min = division_min.or_else(|| {
                        number(&types[0])
                            .filter(|(s, _, f)| *s && !*f)
                            .map(|(_, bits, _)| -(1i128 << (bits - 1)))
                    });
                    if b == 0 || min == Some(a) && b == -1 {
                        return Err(error(
                            153,
                            "ILLEGAL_DIVISION",
                            "Division by zero or minimal signed number by minus one",
                        ));
                    }
                    if op == Op::IntDiv { a / b } else { a % b }
                }
                _ => return Err(ChError::unsupported(op.name())),
            }
        }
    };
    result_int(n, result)
}

/// ClickHouse CAST wraps numeric narrowing; string conversions stay checked.
pub(super) fn cast(v: &ScalarValue, t: &ChType) -> Result<ScalarValue, ChError> {
    if v.is_null() {
        if super::types::nullable(t) {
            return null(t);
        }
        return Err(error(
            349,
            "CANNOT_INSERT_NULL_IN_ORDINARY_COLUMN",
            "Cannot convert NULL to non-nullable type",
        ));
    }
    if number(t).is_some_and(|(_, _, f)| !f) {
        if let Some(n) = int(v) {
            return result_int(n, t);
        }
        if let Some(n) = float(v) {
            return result_int(n as i128, t);
        }
    }
    if matches!(v, ScalarValue::Utf8(Some(_)))
        && matches!(base(t), ChType::Enum8(_) | ChType::Enum16(_))
    {
        return convert_text(v, t);
    }
    v.cast_to(&loams_house_types::arrow::data_type(t)?.0)
        .map_err(|e| ChError::cannot_convert_type(e.to_string()))
}

fn sign_cast(value: i128, bits: u8) -> i128 {
    let shift = 128 - u32::from(bits);
    (value << shift) >> shift
}
