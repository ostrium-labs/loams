//! What the composite-type functions share (plan HE1 Task 32): a ClickHouse
//! function over arrays, maps and tuples as a DataFusion `ScalarUDF`
//! ([`Composite`]), ClickHouse's least supertype ([`supertype`]), Arrow
//! re-layout and list helpers ([`values`]) and value equality and order on
//! Arrow's row format ([`rows`]).
//!
//! **Types.** Every argument's ClickHouse type is read from its field's
//! `loams.ch.type` metadata (or inferred from Arrow, R21.3), and every result
//! field carries its own, at every nesting level, so `Array(LowCardinality(
//! String))` stays that through `arrayElement`, `arraySlice` and the rest.
//!
//! **NULL and `LowCardinality`.** ClickHouse wraps most functions in its
//! default implementations (`IFunction::useDefaultImplementationFor{Nulls,
//! LowCardinalityColumns}`, `src/Functions/IFunction.cpp`, Apache-2.0):
//!
//! * an argument of type `Nullable(Nothing)` (a literal `NULL`) makes the
//!   result `Nullable(Nothing)`;
//! * a `Nullable(T)` argument makes the result `Nullable`, `NULL` on the rows
//!   where an argument is `NULL`; the kernel sees `T`;
//! * when every non-constant argument is `LowCardinality`, so is the result.
//!
//! Functions that look at `NULL`s themselves (`has`, `indexOf`, `array`,
//! `tuple`, `map`) opt out with [`Composite::own_nulls`].

pub mod rows;
pub mod supertype;
pub mod values;

use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, make_array};
use datafusion::arrow::buffer::NullBuffer;
use datafusion::arrow::datatypes::{DataType, FieldRef};
use datafusion::common::{DataFusionError, Result, ScalarValue};
use datafusion::logical_expr::{
    ColumnarValue, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature,
    Volatility,
};
use loams_house_sql::Registry;
use loams_house_sql::functions::internal_name;
use loams_house_types::arrow::{from_arrow, nullable, to_arrow};
use loams_house_types::{ChError, ChType};

pub use values::coerce_to;

/// How a function types its result from its arguments' ClickHouse types and,
/// for constant arguments, their values.
pub type Typer =
    Arc<dyn Fn(&[ChType], &[Option<&ScalarValue>]) -> Result<ChType, ChError> + Send + Sync>;

/// A function's work over one batch.
pub type Kernel = Arc<dyn Fn(&Call<'_>) -> Result<ArrayRef, ChError> + Send + Sync>;

/// One call of a kernel.
#[derive(Debug)]
pub struct Call<'a> {
    /// The function's ClickHouse name, for messages.
    pub name: &'a str,
    /// The arguments, each [`Self::rows`] long. A top-level dictionary
    /// (`LowCardinality`) is decoded; nested ones are kept.
    pub args: &'a [ArrayRef],
    /// The constant value of each argument that is constant.
    pub consts: &'a [Option<ScalarValue>],
    /// The arguments' ClickHouse types. Under the default `NULL` handling the
    /// top-level `Nullable` and `LowCardinality` are taken off.
    pub types: &'a [ChType],
    /// How many rows to produce.
    pub rows: usize,
    /// The result's type, as the typer gave it.
    pub ret: &'a ChType,
}

impl Call<'_> {
    /// The constant integer value of argument `i`, if it is one.
    pub fn const_int(&self, i: usize) -> Option<i64> {
        self.consts.get(i)?.as_ref().and_then(scalar_i64)
    }
}

/// An integer scalar as `i64` (a `UInt64` above `i64::MAX` saturates).
pub fn scalar_i64(s: &ScalarValue) -> Option<i64> {
    Some(match s {
        ScalarValue::Int8(Some(v)) => i64::from(*v),
        ScalarValue::Int16(Some(v)) => i64::from(*v),
        ScalarValue::Int32(Some(v)) => i64::from(*v),
        ScalarValue::Int64(Some(v)) => *v,
        ScalarValue::UInt8(Some(v)) => i64::from(*v),
        ScalarValue::UInt16(Some(v)) => i64::from(*v),
        ScalarValue::UInt32(Some(v)) => i64::from(*v),
        ScalarValue::UInt64(Some(v)) => i64::try_from(*v).unwrap_or(i64::MAX),
        ScalarValue::Boolean(Some(v)) => i64::from(*v),
        ScalarValue::Dictionary(_, inner) => return scalar_i64(inner),
        _ => return None,
    })
}

/// How a function treats `NULL` arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nulls {
    /// ClickHouse's default implementation (see the module docs).
    Default,
    /// The kernel sees the full types and the `NULL`s.
    Own,
}

/// A ClickHouse function over composite types as a DataFusion scalar
/// function.
pub struct Composite {
    name: String,
    internal: String,
    signature: Signature,
    typer: Typer,
    kernel: Kernel,
    nulls: Nulls,
    low_cardinality: bool,
}

impl fmt::Debug for Composite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Composite")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Composite {
    fn eq(&self, other: &Self) -> bool {
        self.internal == other.internal
    }
}

impl Eq for Composite {}

impl Hash for Composite {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.internal.hash(state);
    }
}

impl Composite {
    /// The function ClickHouse calls `name`, typed by `typer` and run by
    /// `kernel`, with ClickHouse's default `NULL` and `LowCardinality`
    /// handling.
    pub fn new(name: &str, typer: Typer, kernel: Kernel) -> Self {
        Self {
            name: name.to_string(),
            internal: internal_name(name),
            signature: Signature::user_defined(Volatility::Immutable),
            typer,
            kernel,
            nulls: Nulls::Default,
            low_cardinality: true,
        }
    }

    /// The same function, seeing `NULL`s and full types itself
    /// (`useDefaultImplementationForNulls` is false in ClickHouse).
    pub fn own_nulls(mut self) -> Self {
        self.nulls = Nulls::Own;
        self.low_cardinality = false;
        self
    }

    /// The same function, never `LowCardinality` in its result.
    pub fn no_low_cardinality(mut self) -> Self {
        self.low_cardinality = false;
        self
    }

    /// The scalar function DataFusion runs.
    pub fn into_udf(self) -> Arc<ScalarUDF> {
        Arc::new(ScalarUDF::new_from_impl(self))
    }

    /// Registers the function under its internal name
    /// ([`internal_name`]).
    pub fn register(self, r: &mut Registry) {
        let key = self.internal.clone();
        r.scalars.insert(key, self.into_udf());
    }

    fn result_type(
        &self,
        types: &[ChType],
        scalars: &[Option<&ScalarValue>],
    ) -> Result<ChType, ChError> {
        if self.nulls == Nulls::Own {
            return (self.typer)(types, scalars);
        }
        if types
            .iter()
            .any(|t| matches!(t.strip_nullable(), ChType::Nothing))
        {
            return Ok(ChType::Nullable(Box::new(ChType::Nothing)));
        }
        let any_nullable = types.iter().any(top_nullable);
        let is_lc = |t: &ChType| matches!(t, ChType::LowCardinality(_));
        let lc = self.low_cardinality
            && types.iter().any(is_lc)
            && types
                .iter()
                .enumerate()
                .all(|(i, t)| is_lc(t) || scalars.get(i).is_some_and(Option::is_some));
        let plain: Vec<ChType> = types.iter().map(strip_top).collect();
        let mut t = (self.typer)(&plain, scalars)?;
        if any_nullable {
            t = nullable(t);
        }
        if lc && !matches!(t, ChType::LowCardinality(_)) && can_be_low_cardinality(&t) {
            t = ChType::LowCardinality(Box::new(t));
        }
        Ok(t)
    }
}

/// Whether a value of `t` itself may be `NULL` at the top level.
fn top_nullable(t: &ChType) -> bool {
    match t {
        ChType::Nullable(_) => true,
        ChType::LowCardinality(inner) => top_nullable(inner),
        _ => false,
    }
}

/// `t` without its top-level `LowCardinality` and `Nullable`.
fn strip_top(t: &ChType) -> ChType {
    match t {
        ChType::Nullable(inner) | ChType::LowCardinality(inner) => strip_top(inner),
        other => other.clone(),
    }
}

/// Whether ClickHouse allows `LowCardinality(t)`.
fn can_be_low_cardinality(t: &ChType) -> bool {
    matches!(
        t.strip_nullable(),
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
            | ChType::Uuid
            | ChType::IPv4
            | ChType::IPv6
    )
}

/// A ClickHouse error carried through DataFusion.
pub fn df(e: ChError) -> DataFusionError {
    DataFusionError::External(Box::new(e))
}

/// An Arrow error as a ClickHouse logical error.
pub fn arrow_err(e: impl fmt::Display) -> ChError {
    ChError::logical_error(e.to_string())
}

/// `42`: the function takes `expected` arguments.
pub fn arity(name: &str, got: usize, expected: &str) -> ChError {
    ChError::number_of_arguments_doesnt_match(format!(
        "Number of arguments for function {name} doesn't match: passed {got}, should be {expected}"
    ))
}

/// `42` unless `min <= got <= max`.
pub fn check_arity(name: &str, got: usize, min: usize, max: usize) -> Result<(), ChError> {
    if got < min || got > max {
        let expected = if min == max {
            min.to_string()
        } else if max == usize::MAX {
            format!("at least {min}")
        } else {
            format!("from {min} to {max}")
        };
        return Err(arity(name, got, &expected));
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

/// `190 SIZES_OF_ARRAYS_DONT_MATCH`.
pub fn sizes_dont_match(name: &str) -> ChError {
    ChError {
        code: 190,
        name: "SIZES_OF_ARRAYS_DONT_MATCH",
        message: format!("Arrays passed to {name} must have equal size"),
    }
}

/// The element type of an `Array` argument, or `43`.
pub fn element_of<'t>(name: &str, types: &'t [ChType], i: usize) -> Result<&'t ChType, ChError> {
    match types.get(i) {
        Some(ChType::Array(inner)) => Ok(inner),
        Some(t) => Err(illegal_type(name, i, t)),
        None => Err(arity(name, types.len(), &format!("at least {}", i + 1))),
    }
}

impl ScalarUDFImpl for Composite {
    fn name(&self) -> &str {
        &self.internal
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn coerce_types(&self, arg_types: &[DataType]) -> Result<Vec<DataType>> {
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
        let t = self
            .result_type(&types, args.scalar_arguments)
            .map_err(df)?;
        Ok(Arc::new(to_arrow(&self.name, &t).map_err(df)?))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let out_type = args.return_field.data_type().clone();
        let ret = from_arrow(&args.return_field).map_err(df)?;
        let full: Vec<ChType> = args
            .arg_fields
            .iter()
            .map(|f| from_arrow(f))
            .collect::<Result<Vec<_>, _>>()
            .map_err(df)?;
        let all_const = !args.args.is_empty()
            && args
                .args
                .iter()
                .all(|a| matches!(a, ColumnarValue::Scalar(_)));
        let rows = if all_const { 1 } else { args.number_rows };
        if self.nulls == Nulls::Default && matches!(ret.strip_nullable(), ChType::Nothing) {
            return Ok(ColumnarValue::Scalar(ScalarValue::try_from(&out_type)?));
        }
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
            let arr = a.to_array(rows)?;
            arrays.push(if self.nulls == Nulls::Default {
                values::decode_dictionary(&arr).map_err(df)?
            } else {
                arr
            });
        }
        let (types, kernel_ret) = match self.nulls {
            Nulls::Own => (full.clone(), ret.clone()),
            Nulls::Default => (full.iter().map(strip_top).collect(), strip_top(&ret)),
        };
        let out = (self.kernel)(&Call {
            name: &self.name,
            args: &arrays,
            consts: &consts,
            types: &types,
            rows,
            ret: &kernel_ret,
        })
        .map_err(df)?;
        let mut out = coerce_to(&out, &out_type).map_err(df)?;
        if self.nulls == Nulls::Default && full.iter().any(top_nullable) {
            let mut mask: Option<NullBuffer> = out.logical_nulls();
            for (a, t) in arrays.iter().zip(&full) {
                if top_nullable(t) {
                    mask = NullBuffer::union(mask.as_ref(), a.logical_nulls().as_ref());
                }
            }
            if let Some(mask) = mask {
                out = with_nulls(&out, mask).map_err(df)?;
            }
        }
        if all_const {
            Ok(ColumnarValue::Scalar(ScalarValue::try_from_array(&out, 0)?))
        } else {
            Ok(ColumnarValue::Array(out))
        }
    }
}

/// `a` with `mask` as its validity. A dictionary keeps its values and takes
/// the mask on its keys.
pub fn with_nulls(a: &ArrayRef, mask: NullBuffer) -> Result<ArrayRef, ChError> {
    let data = a.to_data();
    let data = data
        .into_builder()
        .nulls(Some(mask))
        .build()
        .map_err(arrow_err)?;
    Ok(make_array(data))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn default_nulls_wrap_the_result() {
        let f = Composite::new(
            "length",
            Arc::new(|_, _| Ok(ChType::UInt64)),
            Arc::new(|_| Err(ChError::logical_error("unused"))),
        );
        let t = f
            .result_type(&[ChType::Nullable(Box::new(ChType::String))], &[None])
            .unwrap();
        assert_eq!(t, ChType::Nullable(Box::new(ChType::UInt64)));
        let t = f
            .result_type(&[ChType::Nullable(Box::new(ChType::Nothing))], &[None])
            .unwrap();
        assert_eq!(t, ChType::Nullable(Box::new(ChType::Nothing)));
        let lc = ChType::LowCardinality(Box::new(ChType::String));
        let t = f.result_type(&[lc], &[None]).unwrap();
        assert_eq!(t, ChType::LowCardinality(Box::new(ChType::UInt64)));
    }

    #[test]
    fn arity_messages_read_as_clickhouse() {
        let e = check_arity("arraySlice", 1, 2, 3).unwrap_err();
        assert_eq!(e.code, 42);
        assert!(e.message.contains("passed 1, should be from 2 to 3"));
    }
}
