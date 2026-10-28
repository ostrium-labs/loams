//! A ClickHouse scalar function as a DataFusion `ScalarUDF` (plan HE1
//! Task 31, R31.1).
//!
//! ClickHouse wraps most functions in three default implementations
//! (`IFunction::useDefaultImplementationFor{Nulls,Nothing,LowCardinality}`,
//! `src/Functions/IFunction.cpp`), and so does [`ChFunction`]:
//!
//! * an argument of type `Nothing` (a literal `NULL`) makes the result
//!   `Nullable(Nothing)`, all `NULL`, without running the function;
//! * a `Nullable` argument makes the result `Nullable`, `NULL` on the rows
//!   where any argument is `NULL` (the kernels propagate `None`);
//! * when every non-constant argument is `LowCardinality`, the result is
//!   `LowCardinality` too (`lower(lc)` is `LowCardinality(String)`,
//!   `length(lc)` is `LowCardinality(UInt64)`), as measured on the oracle.
//!
//! The kernel itself sees plain arrays: dictionaries decoded, constants kept
//! as [`Arg::Const`], and the arguments' ClickHouse types with `Nullable` and
//! `LowCardinality` taken off. Every result field carries its ClickHouse type
//! in `loams.ch.type`, so the output names it exactly (`UUID`, not
//! `FixedString(16)`).

use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, FixedSizeBinaryArray, GenericListArray, MapArray, StringArray,
};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{DataType, FieldRef};
use datafusion::common::{DataFusionError, Result, ScalarValue};
use datafusion::logical_expr::{
    ColumnarValue, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature,
    Volatility,
};
use loams_house_sql::Registry;
use loams_house_types::arrow::{from_arrow, nullable, to_arrow};
use loams_house_types::{ChError, ChType};

/// How a function types its result: from its arguments' types (`Nullable`
/// and `LowCardinality` taken off) and their values when constant.
pub type Typer =
    Arc<dyn Fn(&[ChType], &[Option<&ScalarValue>]) -> Result<ChType, ChError> + Send + Sync>;

/// A function's work over one batch.
pub type Kernel = Arc<dyn Fn(&Call<'_>) -> Result<ArrayRef, ChError> + Send + Sync>;

/// One argument as a kernel sees it.
#[derive(Debug, Clone)]
pub enum Arg {
    /// A constant (`NULL` is a typed null scalar).
    Const(ScalarValue),
    /// A column, decoded from any dictionary, strings as `Utf8`.
    Col(ArrayRef),
}

/// A call of a kernel.
#[derive(Debug)]
pub struct Call<'a> {
    /// The function's name, for messages.
    pub name: &'a str,
    /// The arguments.
    pub args: &'a [Arg],
    /// Their ClickHouse types, without `Nullable` or `LowCardinality`.
    pub types: &'a [ChType],
    /// How many rows to produce.
    pub rows: usize,
    /// The result's type, without `Nullable` or `LowCardinality`.
    pub ret: &'a ChType,
}

/// A ClickHouse function as a DataFusion scalar function.
pub struct ChFunction {
    name: String,
    signature: Signature,
    typer: Typer,
    kernel: Kernel,
    low_cardinality: bool,
}

impl fmt::Debug for ChFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChFunction")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ChFunction {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for ChFunction {}

impl Hash for ChFunction {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl ChFunction {
    /// An immutable function `name` typed by `typer` and run by `kernel`.
    pub fn new(name: &str, typer: Typer, kernel: Kernel) -> Self {
        Self {
            name: name.to_string(),
            signature: Signature::user_defined(Volatility::Immutable),
            typer,
            kernel,
            low_cardinality: true,
        }
    }

    /// The same function, volatile (`generateUUIDv4`): never folded, run
    /// once per row.
    pub fn volatile(mut self) -> Self {
        self.signature = Signature::user_defined(Volatility::Volatile);
        self.low_cardinality = false;
        self
    }

    /// Registers the function under its name.
    pub fn register(self, r: &mut Registry) {
        let name = self.name.clone();
        r.scalars
            .insert(name, Arc::new(ScalarUDF::new_from_impl(self)));
    }
}

/// A ClickHouse error carried through DataFusion.
pub fn df(e: ChError) -> DataFusionError {
    DataFusionError::External(Box::new(e))
}

/// `42`: the function takes `expected` arguments.
pub fn arity(name: &str, got: usize, expected: &str) -> ChError {
    ChError::number_of_arguments_doesnt_match(format!(
        "Number of arguments for function {name} doesn't match: passed {got}, should be {expected}"
    ))
}

/// `43`: argument `i` (0-based) has a type the function does not take.
pub fn illegal_type(name: &str, i: usize, t: &ChType) -> ChError {
    ChError::illegal_type_of_argument(format!(
        "Illegal type {t} of argument {} of function {name}",
        i + 1
    ))
}

/// `48`: a type ClickHouse takes here but the engine does not yet.
pub fn unsupported_type(name: &str, t: &ChType) -> ChError {
    ChError::unsupported(format!("{name} of an argument of type {t}"))
}

/// `48`: a byte-wise result that is not UTF-8. The engine's `String` is
/// Arrow `Utf8` (R21.3), so a result that splits a UTF-8 character cannot be
/// held (R31.2).
pub fn not_utf8(name: &str) -> ChError {
    ChError::unsupported(format!(
        "A result of {name} that splits a UTF-8 character (the engine's strings are UTF-8)"
    ))
}

impl ScalarUDFImpl for ChFunction {
    fn name(&self) -> &str {
        &self.name
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
        let full = self
            .result_type(&types, args.scalar_arguments)
            .map_err(df)?;
        Ok(Arc::new(to_arrow(&self.name, &full).map_err(df)?))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let out_type = args.return_field.data_type().clone();
        if out_type == DataType::Null {
            return Ok(ColumnarValue::Scalar(ScalarValue::Null));
        }
        let full = from_arrow(&args.return_field).map_err(df)?;
        let types = args
            .arg_fields
            .iter()
            .map(|f| from_arrow(f).map(|t| t.base().clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(df)?;
        let all_const = !args.args.is_empty()
            && args
                .args
                .iter()
                .all(|a| matches!(a, ColumnarValue::Scalar(_)));
        let rows = if all_const { 1 } else { args.number_rows };
        let plain = args
            .args
            .into_iter()
            .map(|a| match a {
                ColumnarValue::Scalar(s) => plain_scalar(s).map(Arg::Const),
                ColumnarValue::Array(a) => plain_array(&a).map(Arg::Col),
            })
            .collect::<Result<Vec<_>>>()?;
        let ret = full.base().clone();
        let mut out = (self.kernel)(&Call {
            name: &self.name,
            args: &plain,
            types: &types,
            rows,
            ret: &ret,
        })
        .map_err(df)?;
        if out.data_type() != &out_type {
            out = cast(&out, &out_type)?;
        }
        if all_const {
            Ok(ColumnarValue::Scalar(ScalarValue::try_from_array(&out, 0)?))
        } else {
            Ok(ColumnarValue::Array(out))
        }
    }
}

impl ChFunction {
    fn result_type(
        &self,
        types: &[ChType],
        scalars: &[Option<&ScalarValue>],
    ) -> Result<ChType, ChError> {
        if types.iter().any(|t| matches!(t.base(), ChType::Nothing)) {
            return Ok(ChType::Nullable(Box::new(ChType::Nothing)));
        }
        let any_nullable = types.iter().any(ChType::is_nullable);
        let is_lc = |t: &ChType| matches!(t, ChType::LowCardinality(_));
        let lc = self.low_cardinality
            && types.iter().any(is_lc)
            && types
                .iter()
                .enumerate()
                .all(|(i, t)| is_lc(t) || scalars.get(i).is_some_and(Option::is_some));
        let base: Vec<ChType> = types.iter().map(|t| t.base().clone()).collect();
        let mut t = (self.typer)(&base, scalars)?;
        if any_nullable {
            t = nullable(t);
        }
        if lc {
            t = ChType::LowCardinality(Box::new(t));
        }
        Ok(t)
    }
}

/// A scalar without its dictionary, strings as `Utf8`.
fn plain_scalar(s: ScalarValue) -> Result<ScalarValue> {
    Ok(match s {
        ScalarValue::Dictionary(_, inner) => plain_scalar(*inner)?,
        ScalarValue::LargeUtf8(v) | ScalarValue::Utf8View(v) => ScalarValue::Utf8(v),
        other => other,
    })
}

/// An array without its dictionary, strings as `Utf8`.
fn plain_array(a: &ArrayRef) -> Result<ArrayRef> {
    Ok(match a.data_type() {
        DataType::Dictionary(_, values) => plain_array(&cast(a, values)?)?,
        DataType::LargeUtf8 | DataType::Utf8View => cast(a, &DataType::Utf8)?,
        _ => Arc::clone(a),
    })
}

/// A ready-made typer: always `t`, after checking the argument count lies in
/// `min..=max`.
pub fn fixed(name: &'static str, min: usize, max: usize, t: ChType) -> Typer {
    Arc::new(move |args, _| {
        check_arity(name, args.len(), min, max)?;
        Ok(t.clone())
    })
}

/// `42` unless `min <= got <= max`.
pub fn check_arity(name: &str, got: usize, min: usize, max: usize) -> Result<(), ChError> {
    if got < min || got > max {
        let expected = if min == max {
            min.to_string()
        } else if max == usize::MAX {
            format!("at least {min}")
        } else {
            format!("{min} to {max}")
        };
        return Err(arity(name, got, &expected));
    }
    Ok(())
}

/// `43` unless argument `i` is a `String`.
pub fn want_string(name: &str, args: &[ChType], i: usize) -> Result<(), ChError> {
    match args.get(i) {
        Some(ChType::String) => Ok(()),
        Some(ChType::FixedString(_)) => Err(unsupported_type(name, &args[i])),
        Some(t) => Err(illegal_type(name, i, t)),
        None => Ok(()),
    }
}

/// `43` unless argument `i` is an integer.
pub fn want_integer(name: &str, args: &[ChType], i: usize) -> Result<(), ChError> {
    match args.get(i) {
        Some(
            ChType::UInt8
            | ChType::UInt16
            | ChType::UInt32
            | ChType::UInt64
            | ChType::Int8
            | ChType::Int16
            | ChType::Int32
            | ChType::Int64,
        )
        | None => Ok(()),
        Some(t) => Err(illegal_type(name, i, t)),
    }
}

/// Strings of one argument, constant or not.
#[derive(Debug)]
pub enum Strs<'a> {
    /// The same value on every row.
    Const(Option<&'a str>),
    /// A column.
    Col(&'a StringArray),
}

impl<'a> Strs<'a> {
    /// The value on `row`.
    pub fn get(&self, row: usize) -> Option<&'a str> {
        match self {
            Strs::Const(v) => *v,
            Strs::Col(a) => (!a.is_null(row)).then(|| a.value(row)),
        }
    }

    /// The constant value, if this argument is constant.
    pub fn constant(&self) -> Option<Option<&'a str>> {
        match self {
            Strs::Const(v) => Some(*v),
            Strs::Col(_) => None,
        }
    }
}

/// Bytes of a `String` or `FixedString` argument.
#[derive(Debug)]
pub enum Bytes<'a> {
    /// A constant.
    Const(Option<&'a [u8]>),
    /// A `String` column.
    Str(&'a StringArray),
    /// A `FixedString` column.
    Fixed(&'a FixedSizeBinaryArray),
}

impl<'a> Bytes<'a> {
    /// The value on `row`.
    pub fn get(&self, row: usize) -> Option<&'a [u8]> {
        match self {
            Bytes::Const(v) => *v,
            Bytes::Str(a) => (!a.is_null(row)).then(|| a.value(row).as_bytes()),
            Bytes::Fixed(a) => (!a.is_null(row)).then(|| a.value(row)),
        }
    }
}

/// Integers of one argument, widened to `i128`.
#[derive(Debug)]
pub enum Ints<'a> {
    /// A constant.
    Const(Option<i128>),
    /// A column.
    Col(&'a dyn Array),
}

fn int_at(a: &dyn Array, row: usize) -> Option<i128> {
    use datafusion::arrow::datatypes as t;
    if a.is_null(row) {
        return None;
    }
    Some(match a.data_type() {
        DataType::Int8 => i128::from(a.as_primitive::<t::Int8Type>().value(row)),
        DataType::Int16 => i128::from(a.as_primitive::<t::Int16Type>().value(row)),
        DataType::Int32 => i128::from(a.as_primitive::<t::Int32Type>().value(row)),
        DataType::Int64 => i128::from(a.as_primitive::<t::Int64Type>().value(row)),
        DataType::UInt8 => i128::from(a.as_primitive::<t::UInt8Type>().value(row)),
        DataType::UInt16 => i128::from(a.as_primitive::<t::UInt16Type>().value(row)),
        DataType::UInt32 => i128::from(a.as_primitive::<t::UInt32Type>().value(row)),
        DataType::UInt64 => i128::from(a.as_primitive::<t::UInt64Type>().value(row)),
        _ => return None,
    })
}

fn scalar_int(s: &ScalarValue) -> Option<i128> {
    Some(match s {
        ScalarValue::Int8(Some(v)) => i128::from(*v),
        ScalarValue::Int16(Some(v)) => i128::from(*v),
        ScalarValue::Int32(Some(v)) => i128::from(*v),
        ScalarValue::Int64(Some(v)) => i128::from(*v),
        ScalarValue::UInt8(Some(v)) => i128::from(*v),
        ScalarValue::UInt16(Some(v)) => i128::from(*v),
        ScalarValue::UInt32(Some(v)) => i128::from(*v),
        ScalarValue::UInt64(Some(v)) => i128::from(*v),
        _ => return None,
    })
}

impl Ints<'_> {
    /// The value on `row`.
    pub fn get(&self, row: usize) -> Option<i128> {
        match self {
            Ints::Const(v) => *v,
            Ints::Col(a) => int_at(*a, row),
        }
    }
}

/// The value of a constant integer argument, if it is one.
pub fn const_int(s: Option<&ScalarValue>) -> Option<i128> {
    s.and_then(scalar_int)
}

impl Call<'_> {
    /// Argument `i` as strings.
    pub fn strs(&self, i: usize) -> Result<Strs<'_>, ChError> {
        match &self.args[i] {
            Arg::Const(ScalarValue::Utf8(v)) => Ok(Strs::Const(v.as_deref())),
            Arg::Const(s) if s.is_null() => Ok(Strs::Const(None)),
            Arg::Col(a) if a.data_type() == &DataType::Utf8 => Ok(Strs::Col(a.as_string::<i32>())),
            _ => Err(illegal_type(self.name, i, &self.types[i])),
        }
    }

    /// Argument `i` as bytes (`String` or `FixedString`).
    pub fn bytes(&self, i: usize) -> Result<Bytes<'_>, ChError> {
        match &self.args[i] {
            Arg::Const(ScalarValue::Utf8(v)) => Ok(Bytes::Const(v.as_deref().map(str::as_bytes))),
            Arg::Const(ScalarValue::FixedSizeBinary(_, v)) => Ok(Bytes::Const(v.as_deref())),
            Arg::Const(s) if s.is_null() => Ok(Bytes::Const(None)),
            Arg::Col(a) if a.data_type() == &DataType::Utf8 => Ok(Bytes::Str(a.as_string::<i32>())),
            Arg::Col(a) if matches!(a.data_type(), DataType::FixedSizeBinary(_)) => {
                Ok(Bytes::Fixed(a.as_fixed_size_binary()))
            }
            _ => Err(illegal_type(self.name, i, &self.types[i])),
        }
    }

    /// Argument `i` as integers.
    pub fn ints(&self, i: usize) -> Result<Ints<'_>, ChError> {
        match &self.args[i] {
            Arg::Const(s) if s.is_null() => Ok(Ints::Const(None)),
            Arg::Const(s) => scalar_int(s)
                .map(|v| Ints::Const(Some(v)))
                .ok_or_else(|| illegal_type(self.name, i, &self.types[i])),
            Arg::Col(a) => Ok(Ints::Col(a.as_ref())),
        }
    }

    /// Argument `i` as an array of [`Self::rows`] rows.
    pub fn array(&self, i: usize) -> Result<ArrayRef, ChError> {
        match &self.args[i] {
            Arg::Const(s) => s
                .to_array_of_size(self.rows)
                .map_err(|e| ChError::logical_error(e.to_string())),
            Arg::Col(a) => Ok(Arc::clone(a)),
        }
    }
}

/// The entries of a `List` or `Map` array: (offsets, values).
pub fn list_parts(a: &dyn Array) -> Option<(Vec<usize>, ArrayRef)> {
    match a.data_type() {
        DataType::List(_) => {
            let l: &GenericListArray<i32> = a.as_list();
            let offsets = l.value_offsets().iter().map(|o| *o as usize).collect();
            Some((offsets, Arc::clone(l.values())))
        }
        DataType::LargeList(_) => {
            let l: &GenericListArray<i64> = a.as_list();
            let offsets = l.value_offsets().iter().map(|o| *o as usize).collect();
            Some((offsets, Arc::clone(l.values())))
        }
        DataType::Map(..) => {
            let m: &MapArray = a.as_map();
            let offsets = m.value_offsets().iter().map(|o| *o as usize).collect();
            let entries: ArrayRef = Arc::new(m.entries().clone());
            Some((offsets, entries))
        }
        _ => None,
    }
}
