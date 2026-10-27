//! A ClickHouse function as a DataFusion `ScalarUDF`, typed by ClickHouse
//! types (plan HE1 Task 33, R33.1).
//!
//! [`ChFn`] types its result from its arguments' ClickHouse types (each
//! argument field's `loams.ch.type`, or the type Arrow's implies) and the
//! values of its constant arguments, and puts the result's ClickHouse type in
//! the result field's metadata, so `toTypeName` and the output formats name it
//! exactly (`Nullable(UInt8)`, `Decimal(18, 2)`, `Enum8(…)`).
//!
//! Two ways to treat `NULL`, as ClickHouse's `IFunction` does
//! (`useDefaultImplementationForNulls`, `src/Functions/IFunction.cpp`):
//!
//! * [`Nulls::Default`]: an argument of type `Nothing` (a literal `NULL`)
//!   makes the result `Nullable(Nothing)`; a `Nullable` argument makes the
//!   result `Nullable`, `NULL` on every row where an argument is. The typer
//!   and the kernel see the arguments' types without `Nullable` and
//!   `LowCardinality`. When every non-constant argument is `LowCardinality`,
//!   the result is too (`toString(lc)` is `LowCardinality(String)`).
//! * [`Nulls::Own`]: the function sees the exact types and handles `NULL`
//!   itself (`if`, `coalesce`, `CAST`, `isNull`).
//!
//! Kernels see full-length arrays with dictionaries decoded. T31's `chudf`
//! does the same for strings; R33.1 records unifying the two once both merge.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, make_array, new_null_array};
use datafusion::arrow::buffer::NullBuffer;
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{DataType, Field, FieldRef};
use datafusion::arrow::error::ArrowError;
use datafusion::common::{DataFusionError, Result, ScalarValue};
use datafusion::logical_expr::{
    ColumnarValue, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature,
    Volatility,
};
use loams_house_sql::Registry;
use loams_house_sql::functions::internal_name;
use loams_house_types::arrow::{data_type, from_arrow, nullable, to_arrow};
use loams_house_types::{ChError, ChType};

/// The zone a value without one of its own is read and written in until
/// `session_timezone` is honoured (plan HE1 Task 37): the oracle's server
/// zone, `UTC` (R33.6).
pub const SESSION_TIMEZONE: &str = "UTC";

/// How a function treats `NULL` arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nulls {
    /// ClickHouse's default implementation for `NULL` and `LowCardinality`.
    Default,
    /// The function handles `Nullable` and `LowCardinality` itself.
    Own,
}

/// What a typer sees.
#[derive(Debug)]
pub struct Sig<'a> {
    /// The function's name, for messages.
    pub name: &'a str,
    /// The arguments' ClickHouse types (without `Nullable` and
    /// `LowCardinality` under [`Nulls::Default`]).
    pub types: &'a [ChType],
    /// The constant arguments' values.
    pub consts: &'a [Option<ScalarValue>],
}

/// What a kernel sees.
#[derive(Debug)]
pub struct Call<'a> {
    /// The function's name, for messages.
    pub name: &'a str,
    /// The arguments, `rows` long each, dictionaries decoded.
    pub args: &'a [ArrayRef],
    /// Their ClickHouse types, as the typer saw them.
    pub types: &'a [ChType],
    /// The constant arguments' values.
    pub consts: &'a [Option<ScalarValue>],
    /// The result's ClickHouse type (without `Nullable` and `LowCardinality`
    /// under [`Nulls::Default`]).
    pub ret: &'a ChType,
    /// How many rows to produce.
    pub rows: usize,
}

/// How a function types its result.
pub type Typer = Arc<dyn Fn(&Sig<'_>) -> Result<ChType, ChError> + Send + Sync>;

/// A function's work over one batch.
pub type Kernel = Arc<dyn Fn(&Call<'_>) -> Result<ArrayRef, ChError> + Send + Sync>;

/// A typer from a closure.
pub fn typer(f: impl Fn(&Sig<'_>) -> Result<ChType, ChError> + Send + Sync + 'static) -> Typer {
    Arc::new(f)
}

/// A kernel from a closure.
pub fn kernel(
    f: impl Fn(&Call<'_>) -> Result<ArrayRef, ChError> + Send + Sync + 'static,
) -> Kernel {
    Arc::new(f)
}

/// A ClickHouse scalar function.
pub struct ChFn {
    name: String,
    signature: Signature,
    nulls: Nulls,
    low_cardinality: bool,
    typer: Typer,
    kernel: Kernel,
}

impl fmt::Debug for ChFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChFn")
            .field("name", &self.name)
            .field("nulls", &self.nulls)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ChFn {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for ChFn {}

impl Hash for ChFn {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl ChFn {
    /// An immutable function `name`.
    pub fn new(name: &str, nulls: Nulls, typer: Typer, kernel: Kernel) -> Self {
        Self {
            name: name.to_string(),
            signature: Signature::user_defined(Volatility::Immutable),
            nulls,
            low_cardinality: true,
            typer,
            kernel,
        }
    }

    /// The same function, volatile (`rand`): never folded.
    pub fn volatile(mut self) -> Self {
        self.signature = Signature::user_defined(Volatility::Volatile);
        self.low_cardinality = false;
        self
    }

    /// The same function, never `LowCardinality` (`toTypeName`).
    pub fn plain(mut self) -> Self {
        self.low_cardinality = false;
        self
    }

    /// The function as a DataFusion UDF.
    pub fn into_udf(self) -> Arc<ScalarUDF> {
        Arc::new(ScalarUDF::new_from_impl(self))
    }

    /// Registers the function under its DataFusion name.
    pub fn register(self, r: &mut Registry) {
        let name = internal_name(&self.name);
        r.scalars.insert(name, self.into_udf());
    }

    /// The ClickHouse result type for arguments of `types`.
    pub fn type_of(
        &self,
        types: &[ChType],
        consts: &[Option<ScalarValue>],
    ) -> Result<ChType, ChError> {
        match self.nulls {
            Nulls::Own => (self.typer)(&Sig {
                name: &self.name,
                types,
                consts,
            }),
            Nulls::Default => {
                let stripped: Vec<ChType> = types.iter().map(|t| t.base().clone()).collect();
                if stripped.contains(&ChType::Nothing) {
                    return Ok(ChType::Nullable(Box::new(ChType::Nothing)));
                }
                let inner = (self.typer)(&Sig {
                    name: &self.name,
                    types: &stripped,
                    consts,
                })?;
                let any_nullable = types.iter().any(ChType::is_nullable);
                let lc = self.low_cardinality
                    && types.iter().any(|t| matches!(t, ChType::LowCardinality(_)))
                    && types
                        .iter()
                        .zip(consts.iter().chain(std::iter::repeat(&None)))
                        .all(|(t, c)| matches!(t, ChType::LowCardinality(_)) || c.is_some());
                let mut ret = if any_nullable && !inner.is_nullable() {
                    nullable(inner)
                } else {
                    inner
                };
                if lc && fits_low_cardinality(ret.base()) {
                    ret = ChType::LowCardinality(Box::new(ret));
                }
                Ok(ret)
            }
        }
    }
}

/// Whether ClickHouse allows `LowCardinality(t)`.
pub fn fits_low_cardinality(t: &ChType) -> bool {
    matches!(
        t,
        ChType::String
            | ChType::FixedString(_)
            | ChType::UInt8
            | ChType::UInt16
            | ChType::UInt32
            | ChType::UInt64
            | ChType::Int8
            | ChType::Int16
            | ChType::Int32
            | ChType::Int64
            | ChType::Float32
            | ChType::Float64
            | ChType::Date
            | ChType::Date32
            | ChType::DateTime(_)
    )
}

impl ScalarUDFImpl for ChFn {
    fn name(&self) -> &str {
        &self.name
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn coerce_types(&self, arg_types: &[DataType]) -> Result<Vec<DataType>> {
        // ClickHouse's own rules decide; DataFusion inserts no casts.
        Ok(arg_types.to_vec())
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Err(DataFusionError::Internal(format!(
            "{} is typed by return_field_from_args",
            self.name
        )))
    }

    fn return_field_from_args(&self, args: ReturnFieldArgs) -> Result<FieldRef> {
        let types = args
            .arg_fields
            .iter()
            .map(|f| from_arrow(f))
            .collect::<Result<Vec<_>, _>>()
            .map_err(df)?;
        let consts: Vec<Option<ScalarValue>> =
            args.scalar_arguments.iter().map(|s| s.cloned()).collect();
        let t = self.type_of(&types, &consts).map_err(df)?;
        Ok(Arc::new(to_arrow(&self.name, &t).map_err(df)?))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let rows = args.number_rows;
        let types = args
            .arg_fields
            .iter()
            .map(|f| from_arrow(f))
            .collect::<Result<Vec<_>, _>>()
            .map_err(df)?;
        let ret_full = from_arrow(&args.return_field).map_err(df)?;
        let consts: Vec<Option<ScalarValue>> = args
            .args
            .iter()
            .map(|a| match a {
                ColumnarValue::Scalar(s) => Some(s.clone()),
                ColumnarValue::Array(_) => None,
            })
            .collect();
        let mut arrays = Vec::with_capacity(args.args.len());
        for a in &args.args {
            arrays.push(decode(&a.to_array(rows)?).map_err(df)?);
        }
        let out = match self.nulls {
            Nulls::Own => (self.kernel)(&Call {
                name: &self.name,
                args: &arrays,
                types: &types,
                consts: &consts,
                ret: &ret_full,
                rows,
            })
            .map_err(df)?,
            Nulls::Default => {
                let inner = ret_full.base().clone();
                if inner == ChType::Nothing {
                    new_null_array(&DataType::Null, rows)
                } else {
                    let stripped: Vec<ChType> = types.iter().map(|t| t.base().clone()).collect();
                    let out = (self.kernel)(&Call {
                        name: &self.name,
                        args: &arrays,
                        types: &stripped,
                        consts: &consts,
                        ret: &inner,
                        rows,
                    })
                    .map_err(df)?;
                    let mut nulls: Option<NullBuffer> = None;
                    for a in &arrays {
                        nulls = NullBuffer::union(nulls.as_ref(), a.logical_nulls().as_ref());
                    }
                    with_nulls(out, nulls).map_err(df)?
                }
            }
        };
        Ok(ColumnarValue::Array(conform(out, &ret_full).map_err(df)?))
    }
}

/// `array` with `DictionaryArray`s decoded to their values.
pub fn decode(array: &ArrayRef) -> Result<ArrayRef, ChError> {
    match array.data_type() {
        DataType::Dictionary(_, values) => cast(array, values).map_err(arrow_error),
        _ => Ok(array.clone()),
    }
}

/// `array` with `nulls` added to its own.
pub fn with_nulls(array: ArrayRef, nulls: Option<NullBuffer>) -> Result<ArrayRef, ChError> {
    let Some(nulls) = nulls else {
        return Ok(array);
    };
    if nulls.null_count() == 0 || array.data_type() == &DataType::Null {
        return Ok(array);
    }
    let data = array.to_data();
    let merged = NullBuffer::union(data.nulls(), Some(&nulls));
    let data = data
        .into_builder()
        .nulls(merged)
        .build()
        .map_err(arrow_error)?;
    Ok(make_array(data))
}

/// `array` in the Arrow layout of ClickHouse type `t` (dictionary-encoded for
/// `LowCardinality`, the zone of `DateTime('tz')`).
pub fn conform(array: ArrayRef, t: &ChType) -> Result<ArrayRef, ChError> {
    let (dt, _) = data_type(t)?;
    if array.data_type() == &dt {
        return Ok(array);
    }
    if array.data_type() == &DataType::Null {
        return Ok(new_null_array(&dt, array.len()));
    }
    cast(&array, &dt).map_err(arrow_error)
}

/// A ClickHouse error carried through DataFusion.
pub fn df(e: ChError) -> DataFusionError {
    DataFusionError::External(Box::new(e))
}

/// An Arrow error as ClickHouse's `LOGICAL_ERROR` (the engine's fault, not
/// the query's).
pub fn arrow_error(e: ArrowError) -> ChError {
    ChError::logical_error(e.to_string())
}

/// `42`: the function takes `expected` arguments.
pub fn arity_error(name: &str, got: usize, expected: &str) -> ChError {
    ChError::number_of_arguments_doesnt_match(format!(
        "Number of arguments for function {name} doesn't match: passed {got}, should be {expected}"
    ))
}

/// Checks that `sig` has `min..=max` arguments.
pub fn arity(sig: &Sig<'_>, min: usize, max: usize) -> Result<(), ChError> {
    let n = sig.types.len();
    if !(min..=max).contains(&n) {
        let expected = if min == max {
            min.to_string()
        } else if max == usize::MAX {
            format!("at least {min}")
        } else {
            format!("from {min} to {max}")
        };
        return Err(arity_error(sig.name, n, &expected));
    }
    Ok(())
}

/// `43`: argument `i` (0-based) has a type the function does not take.
pub fn illegal_type(name: &str, i: usize, t: &ChType) -> ChError {
    ChError::illegal_type_of_argument(format!(
        "Illegal type {t} of argument {} of function {name}",
        i + 1
    ))
}

/// `44 ILLEGAL_COLUMN`: argument `i` must be a constant.
pub fn not_constant(name: &str, i: usize) -> ChError {
    ChError {
        code: 44,
        name: "ILLEGAL_COLUMN",
        message: format!("Argument {} of function {name} must be constant", i + 1),
    }
}

/// A constant string argument's value.
pub fn const_str(c: Option<&ScalarValue>) -> Option<String> {
    match c? {
        ScalarValue::Utf8(Some(s))
        | ScalarValue::LargeUtf8(Some(s))
        | ScalarValue::Utf8View(Some(s)) => Some(s.clone()),
        ScalarValue::Dictionary(_, inner) => const_str(Some(inner.as_ref())),
        _ => None,
    }
}

/// A constant integer argument's value.
pub fn const_int(c: Option<&ScalarValue>) -> Option<i128> {
    Some(match c? {
        ScalarValue::Int8(Some(v)) => i128::from(*v),
        ScalarValue::Int16(Some(v)) => i128::from(*v),
        ScalarValue::Int32(Some(v)) => i128::from(*v),
        ScalarValue::Int64(Some(v)) => i128::from(*v),
        ScalarValue::UInt8(Some(v)) => i128::from(*v),
        ScalarValue::UInt16(Some(v)) => i128::from(*v),
        ScalarValue::UInt32(Some(v)) => i128::from(*v),
        ScalarValue::UInt64(Some(v)) => i128::from(*v),
        ScalarValue::Boolean(Some(v)) => i128::from(*v),
        ScalarValue::Dictionary(_, inner) => return const_int(Some(inner.as_ref())),
        _ => return None,
    })
}

/// A field of ClickHouse type `t` (for tests and helpers that build plans).
pub fn field(name: &str, t: &ChType) -> Result<Field, ChError> {
    to_arrow(name, t)
}

/// Whether `t` is one of the integer types the engine stores.
pub fn is_integer(t: &ChType) -> bool {
    matches!(
        t,
        ChType::UInt8
            | ChType::UInt16
            | ChType::UInt32
            | ChType::UInt64
            | ChType::Int8
            | ChType::Int16
            | ChType::Int32
            | ChType::Int64
    )
}

/// Whether `t` is a float type.
pub fn is_float(t: &ChType) -> bool {
    matches!(t, ChType::Float32 | ChType::Float64)
}

/// Whether `t` is a number: an integer, a float, `Bool` or a `Decimal`.
pub fn is_number(t: &ChType) -> bool {
    is_integer(t) || is_float(t) || matches!(t, ChType::Bool | ChType::Decimal { .. })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use datafusion::arrow::array::UInt8Array;

    fn ident() -> ChFn {
        ChFn::new(
            "identityForTest",
            Nulls::Default,
            typer(|s| Ok(s.types[0].clone())),
            kernel(|c| Ok(c.args[0].clone())),
        )
    }

    #[test]
    fn default_nulls_wrap_the_result() {
        let f = ident();
        let t = f
            .type_of(&[ChType::Nullable(Box::new(ChType::UInt8))], &[None])
            .unwrap();
        assert_eq!(t, ChType::Nullable(Box::new(ChType::UInt8)));
        let t = f
            .type_of(&[ChType::Nullable(Box::new(ChType::Nothing))], &[None])
            .unwrap();
        assert_eq!(t, ChType::Nullable(Box::new(ChType::Nothing)));
        let t = f
            .type_of(&[ChType::LowCardinality(Box::new(ChType::String))], &[None])
            .unwrap();
        assert_eq!(t, ChType::LowCardinality(Box::new(ChType::String)));
    }

    #[test]
    fn nulls_merge_into_the_result() {
        let a: ArrayRef = Arc::new(UInt8Array::from(vec![1, 2, 3]));
        let mask = NullBuffer::from(vec![true, false, true]);
        let out = with_nulls(a, Some(mask)).unwrap();
        assert!(out.is_null(1));
        assert!(out.is_valid(0));
    }

    #[test]
    fn constants_read_back() {
        assert_eq!(
            const_str(Some(&ScalarValue::Utf8(Some("UInt8".into())))).as_deref(),
            Some("UInt8")
        );
        assert_eq!(const_int(Some(&ScalarValue::Int64(Some(-3)))), Some(-3));
        assert_eq!(const_int(Some(&ScalarValue::Utf8(None))), None);
    }

    #[test]
    fn arity_errors_are_42() {
        let types = [ChType::UInt8];
        let sig = Sig {
            name: "f",
            types: &types,
            consts: &[None],
        };
        assert_eq!(arity(&sig, 2, 3).unwrap_err().code, 42);
        assert!(arity(&sig, 1, 1).is_ok());
    }
}
