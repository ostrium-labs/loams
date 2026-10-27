//! `intDiv(a, b)` and `intDivOrZero(a, b)`: integer division truncated toward
//! zero, in the type ClickHouse gives it: signed when either argument is,
//! with the dividend's width (`intDiv(UInt8, Int64)` is `Int8`; a float or
//! decimal dividend is the integer of its width). The quotient wraps into
//! that type (`intDiv(toUInt8(200), -1)` is `56`). Ported from ClickHouse's
//! `src/Functions/intDiv.cpp` and `NumberTraits::ResultOfIntegerDivision`
//! (Apache-2.0, see `NOTICE`); measured in `corpus/he1/24`.

use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, PrimitiveArray};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{
    DataType, Field, FieldRef, Float64Type, Int8Type, Int16Type, Int32Type, Int64Type, UInt8Type,
    UInt16Type, UInt32Type, UInt64Type,
};
use datafusion::common::{DataFusionError, Result};
use datafusion::logical_expr::{
    ColumnarValue, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
};
use loams_house_types::ChError;

/// `intDiv` (`or_zero = false`) or `intDivOrZero`.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct IntDiv {
    or_zero: bool,
    signature: Signature,
}

impl IntDiv {
    /// The function; `or_zero` answers 0 where `intDiv` fails.
    pub fn new(or_zero: bool) -> Self {
        Self {
            or_zero,
            signature: Signature::any(2, Volatility::Immutable),
        }
    }
}

fn shape(dt: &DataType) -> Option<(bool, bool, u8)> {
    // (signed, float or decimal, width in bytes)
    Some(match dt {
        DataType::Boolean | DataType::UInt8 => (false, false, 1),
        DataType::UInt16 => (false, false, 2),
        DataType::UInt32 => (false, false, 4),
        DataType::UInt64 => (false, false, 8),
        DataType::Int8 => (true, false, 1),
        DataType::Int16 => (true, false, 2),
        DataType::Int32 => (true, false, 4),
        DataType::Int64 => (true, false, 8),
        DataType::Float32 => (true, true, 4),
        DataType::Float64 => (true, true, 8),
        DataType::Decimal128(p, _) if *p <= 9 => (true, true, 4),
        DataType::Decimal128(p, _) if *p <= 18 => (true, true, 8),
        DataType::Dictionary(_, v) => return shape(v),
        _ => return None,
    })
}

fn illegal(a: &DataType, b: &DataType, name: &str) -> DataFusionError {
    DataFusionError::External(Box::new(ChError::illegal_type_of_argument(format!(
        "Illegal types {a} and {b} of arguments of function {name}"
    ))))
}

fn ch(e: ChError) -> DataFusionError {
    DataFusionError::External(Box::new(e))
}

/// The result type of `intDiv(a, b)`.
pub fn result_type(a: &DataType, b: &DataType) -> Option<DataType> {
    let (sa, _, size) = shape(a)?;
    let (sb, _, _) = shape(b)?;
    Some(match (sa || sb, size) {
        (false, 1) => DataType::UInt8,
        (false, 2) => DataType::UInt16,
        (false, 4) => DataType::UInt32,
        (false, _) => DataType::UInt64,
        (true, 1) => DataType::Int8,
        (true, 2) => DataType::Int16,
        (true, 4) => DataType::Int32,
        (true, _) => DataType::Int64,
    })
}

/// For integer operands of which at least one is signed, the widths (in
/// bytes) of the signed types ClickHouse divides them as: the dividend's own,
/// the divisor's or the dividend's, whichever is wider.
fn mixed_sign_widths(a: &DataType, b: &DataType) -> Option<(u8, u8)> {
    let (sa, fa, wa) = shape(a)?;
    let (sb, fb, wb) = shape(b)?;
    if fa || fb || !(sa || sb) {
        return None;
    }
    Some((wa, wa.max(wb)))
}

/// `v` wrapped into the signed integer of `bytes` bytes.
fn wrap_signed(v: i128, bytes: u8) -> i128 {
    let bits = 8 * u32::from(bytes);
    let m = v & ((1i128 << bits) - 1);
    if m >= 1i128 << (bits - 1) {
        m - (1i128 << bits)
    } else {
        m
    }
}

/// The smallest signed integer of `bytes` bytes.
fn min_signed(bytes: u8) -> i128 {
    -(1i128 << (8 * u32::from(bytes) - 1))
}

fn as_i128(a: &ArrayRef) -> Result<Vec<Option<i128>>> {
    macro_rules! ints {
        ($t:ty) => {
            a.as_primitive::<$t>()
                .iter()
                .map(|v| v.map(i128::from))
                .collect()
        };
    }
    Ok(match a.data_type() {
        DataType::UInt8 => ints!(UInt8Type),
        DataType::UInt16 => ints!(UInt16Type),
        DataType::UInt32 => ints!(UInt32Type),
        DataType::UInt64 => ints!(UInt64Type),
        DataType::Int8 => ints!(Int8Type),
        DataType::Int16 => ints!(Int16Type),
        DataType::Int32 => ints!(Int32Type),
        DataType::Int64 => ints!(Int64Type),
        other => {
            return Err(ch(ChError::logical_error(format!(
                "intDiv over {other} as integers"
            ))));
        }
    })
}

impl ScalarUDFImpl for IntDiv {
    fn name(&self) -> &str {
        if self.or_zero {
            "intDivOrZero"
        } else {
            "intDiv"
        }
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        result_type(&args[0], &args[1]).ok_or_else(|| illegal(&args[0], &args[1], self.name()))
    }

    /// `Nullable` only when an argument is (`intDiv(UInt8, Int8)` is `Int8`).
    fn return_field_from_args(&self, args: ReturnFieldArgs) -> Result<FieldRef> {
        let types: Vec<DataType> = args
            .arg_fields
            .iter()
            .map(|f| f.data_type().clone())
            .collect();
        let nullable = args.arg_fields.iter().any(|f| f.is_nullable());
        Ok(Arc::new(Field::new(
            self.name(),
            self.return_type(&types)?,
            nullable,
        )))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let rows = args.number_rows;
        let out_type = self.return_type(&[args.args[0].data_type(), args.args[1].data_type()])?;
        let arrays = ColumnarValue::values_to_arrays(&args.args)?;
        let mut cols = Vec::with_capacity(2);
        let mut float = false;
        for a in &arrays {
            let a = match a.data_type() {
                DataType::Dictionary(_, v) => cast(a, v)?,
                DataType::Boolean => cast(a, &DataType::UInt8)?,
                _ => a.clone(),
            };
            float |= shape(a.data_type()).is_some_and(|(_, f, _)| f);
            cols.push(a);
        }
        // Each quotient as an i128, then wrapped into the result type.
        let mut quotients: Vec<Option<i128>> = Vec::with_capacity(rows);
        if float {
            let x = cast(&cols[0], &DataType::Float64)?;
            let y = cast(&cols[1], &DataType::Float64)?;
            let (x, y) = (
                x.as_primitive::<Float64Type>(),
                y.as_primitive::<Float64Type>(),
            );
            for i in 0..rows {
                if x.is_null(i) || y.is_null(i) {
                    quotients.push(None);
                    continue;
                }
                let q = (x.value(i) / y.value(i)).trunc();
                if !q.is_finite() || q.abs() >= 1.8e19 {
                    if self.or_zero {
                        quotients.push(Some(0));
                        continue;
                    }
                    return Err(ch(ChError {
                        code: 153,
                        name: "ILLEGAL_DIVISION",
                        message: "Cannot perform integer division, because it will produce infinite or too large number".into(),
                    }));
                }
                quotients.push(Some(q as i128));
            }
        } else {
            let signs = mixed_sign_widths(cols[0].data_type(), cols[1].data_type());
            let (x, y) = (as_i128(&cols[0])?, as_i128(&cols[1])?);
            for (a, b) in x.into_iter().zip(y) {
                let (Some(a), Some(b)) = (a, b) else {
                    quotients.push(None);
                    continue;
                };
                // ClickHouse divides a signed and an unsigned operand as
                // signed numbers: the dividend as the signed type of its own
                // width, the divisor as the signed type of the wider of the
                // two (`DivideIntegralImpl`); `intDiv(toUInt8(255), -128)`
                // is `0` and `intDiv(toInt8(-128), toUInt16(65535))` is `153`.
                let (a, b) = match signs {
                    Some((wa, wb)) => (wrap_signed(a, wa), wrap_signed(b, wb)),
                    None => (a, b),
                };
                let fails = b == 0
                    || (signs.is_some() && b == -1 && a == min_signed(signs.map_or(8, |s| s.0)));
                if fails {
                    if self.or_zero {
                        quotients.push(Some(0));
                        continue;
                    }
                    return Err(ch(ChError {
                        code: 153,
                        name: "ILLEGAL_DIVISION",
                        message: if b == 0 {
                            "Division by zero".into()
                        } else {
                            "Division of minimal signed number by minus one".into()
                        },
                    }));
                }
                quotients.push(Some(a / b));
            }
        }
        macro_rules! wrap {
            ($t:ty, $n:ty) => {
                Arc::new(
                    quotients
                        .iter()
                        .map(|q| q.map(|v| v as $n))
                        .collect::<PrimitiveArray<$t>>(),
                ) as ArrayRef
            };
        }
        let out = match out_type {
            DataType::UInt8 => wrap!(UInt8Type, u8),
            DataType::UInt16 => wrap!(UInt16Type, u16),
            DataType::UInt32 => wrap!(UInt32Type, u32),
            DataType::UInt64 => wrap!(UInt64Type, u64),
            DataType::Int8 => wrap!(Int8Type, i8),
            DataType::Int16 => wrap!(Int16Type, i16),
            DataType::Int32 => wrap!(Int32Type, i32),
            _ => wrap!(Int64Type, i64),
        };
        Ok(ColumnarValue::Array(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_signs_divide_as_signed() {
        assert_eq!(wrap_signed(255, 1), -1);
        assert_eq!(wrap_signed(65535, 2), -1);
        assert_eq!(wrap_signed(127, 1), 127);
        assert_eq!(min_signed(1), -128);
        // intDiv(UInt8, Int8): both as Int8.
        assert_eq!(
            mixed_sign_widths(&DataType::UInt8, &DataType::Int8),
            Some((1, 1))
        );
        // intDiv(Int8, UInt16): the divisor as Int16 (65535 is -1).
        assert_eq!(
            mixed_sign_widths(&DataType::Int8, &DataType::UInt16),
            Some((1, 2))
        );
        // Both unsigned: as they are.
        assert_eq!(mixed_sign_widths(&DataType::UInt8, &DataType::UInt64), None);
    }

    #[test]
    fn result_types_as_measured() {
        assert_eq!(
            result_type(&DataType::UInt8, &DataType::Int64),
            Some(DataType::Int8)
        );
        assert_eq!(
            result_type(&DataType::Float32, &DataType::UInt8),
            Some(DataType::Int32)
        );
        assert_eq!(
            result_type(&DataType::UInt64, &DataType::UInt8),
            Some(DataType::UInt64)
        );
        assert_eq!(
            result_type(&DataType::Decimal128(9, 2), &DataType::UInt8),
            Some(DataType::Int32)
        );
    }
}
