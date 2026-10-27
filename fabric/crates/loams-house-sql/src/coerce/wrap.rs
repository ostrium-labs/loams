//! Integer conversions that wrap, as ClickHouse's do (plan HE1 Task 24).
//!
//! An arithmetic operand is converted to the operator's computing type before
//! the `BinaryExpr` runs, and ClickHouse's conversion wraps
//! (`toUInt64(18446744073709551615) - 1` computes in `Int64`, where the
//! operand is `-1`). Arrow's `CAST` refuses a value that does not fit, so a
//! conversion that can lose a value (to a narrower integer, or between
//! signedness) is this function instead: `__ch_wrap_<type>(x)`. Every other
//! conversion stays a `CAST`.

use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, PrimitiveArray};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{
    DataType, Field, FieldRef, Int8Type, Int16Type, Int32Type, Int64Type, UInt8Type, UInt16Type,
    UInt32Type, UInt64Type,
};
use datafusion::common::{DataFusionError, Result};
use datafusion::logical_expr::expr::ScalarFunction;
use datafusion::logical_expr::{
    Cast, ColumnarValue, Expr, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl,
    Signature, Volatility,
};

/// (signed, width in bytes) of an integer type.
fn integer(dt: &DataType) -> Option<(bool, u8)> {
    Some(match dt {
        DataType::UInt8 => (false, 1),
        DataType::UInt16 => (false, 2),
        DataType::UInt32 => (false, 4),
        DataType::UInt64 => (false, 8),
        DataType::Int8 => (true, 1),
        DataType::Int16 => (true, 2),
        DataType::Int32 => (true, 4),
        DataType::Int64 => (true, 8),
        _ => return None,
    })
}

/// Whether converting `from` to `to` can lose a value (both integers).
pub fn lossy(from: &DataType, to: &DataType) -> bool {
    let (Some((sf, wf)), Some((st, wt))) = (integer(from), integer(to)) else {
        return false;
    };
    match (sf, st) {
        (false, false) | (true, true) => wt < wf,
        (false, true) => wt <= wf,
        (true, false) => true,
    }
}

/// `e` converted to `to`, wrapping when [`lossy`] from `from`.
pub fn convert(e: Expr, from: &DataType, to: &DataType) -> Expr {
    if lossy(from, to) {
        Expr::ScalarFunction(ScalarFunction::new_udf(
            Arc::new(ScalarUDF::new_from_impl(Wrap::new(to.clone()))),
            vec![e],
        ))
    } else {
        Expr::Cast(Cast::new(Box::new(e), to.clone()))
    }
}

/// `__ch_wrap_<type>(x)`: `x` as `to`, wrapped.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Wrap {
    to: DataType,
    name: String,
    signature: Signature,
}

impl Wrap {
    /// The conversion to `to`.
    pub fn new(to: DataType) -> Self {
        Self {
            name: format!("__ch_wrap_{}", to.to_string().to_lowercase()),
            to,
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

fn values(a: &ArrayRef) -> Result<Vec<Option<i128>>> {
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
        DataType::Boolean => values(&cast(a, &DataType::UInt8)?)?,
        DataType::Dictionary(_, v) => values(&cast(a, v)?)?,
        other => {
            return Err(DataFusionError::Internal(format!(
                "a wrapping integer conversion of {other}"
            )));
        }
    })
}

/// `values` wrapped into `to` (`as` keeps the low bits).
pub fn wrapped(values: &[Option<i128>], to: &DataType) -> Result<ArrayRef> {
    macro_rules! to {
        ($t:ty, $n:ty) => {
            Arc::new(
                values
                    .iter()
                    .map(|v| v.map(|v| v as $n))
                    .collect::<PrimitiveArray<$t>>(),
            ) as ArrayRef
        };
    }
    Ok(match to {
        DataType::UInt8 => to!(UInt8Type, u8),
        DataType::UInt16 => to!(UInt16Type, u16),
        DataType::UInt32 => to!(UInt32Type, u32),
        DataType::UInt64 => to!(UInt64Type, u64),
        DataType::Int8 => to!(Int8Type, i8),
        DataType::Int16 => to!(Int16Type, i16),
        DataType::Int32 => to!(Int32Type, i32),
        DataType::Int64 => to!(Int64Type, i64),
        other => {
            return Err(DataFusionError::Internal(format!(
                "a wrapping integer conversion to {other}"
            )));
        }
    })
}

impl ScalarUDFImpl for Wrap {
    fn name(&self) -> &str {
        &self.name
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.to.clone())
    }

    fn return_field_from_args(&self, args: ReturnFieldArgs) -> Result<FieldRef> {
        let nullable = args.arg_fields.iter().any(|f| f.is_nullable());
        Ok(Arc::new(Field::new(self.name(), self.to.clone(), nullable)))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let arrays = ColumnarValue::values_to_arrays(&args.args)?;
        let v = values(&arrays[0])?;
        Ok(ColumnarValue::Array(wrapped(&v, &self.to)?))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn only_conversions_that_can_lose_a_value_wrap() {
        assert!(lossy(&DataType::UInt64, &DataType::Int64));
        assert!(lossy(&DataType::Int8, &DataType::UInt64));
        assert!(lossy(&DataType::UInt16, &DataType::UInt8));
        assert!(!lossy(&DataType::UInt8, &DataType::Int16));
        assert!(!lossy(&DataType::Int8, &DataType::Int64));
        assert!(!lossy(&DataType::UInt8, &DataType::Float64));
    }

    #[test]
    fn values_wrap_as_clickhouse() {
        let out = wrapped(&[Some(i128::from(u64::MAX)), None], &DataType::Int64).unwrap();
        let out = out.as_primitive::<Int64Type>();
        assert_eq!(out.value(0), -1);
        assert!(out.is_null(1));
        let out = wrapped(&[Some(-1)], &DataType::UInt8).unwrap();
        assert_eq!(out.as_primitive::<UInt8Type>().value(0), 255);
    }
}
