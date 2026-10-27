//! [`ChFunction`]: one ClickHouse scalar function as a DataFusion
//! `ScalarUDF`, typed and run the way ClickHouse's `IFunction` is.
//!
//! * The result type comes from the arguments' ClickHouse types (each field's
//!   `loams.ch.type`, else inferred from Arrow) and the constant arguments,
//!   and the result field carries it, so `DateTime('Asia/Kolkata')`,
//!   `DateTime64(3)` and `IntervalDay` reach the output exactly.
//! * NULLs as ClickHouse's default implementation treats them: a `NULL`
//!   literal argument (`Nullable(Nothing)`) makes the result
//!   `Nullable(Nothing)`, a `Nullable` argument makes it `Nullable`, and a row
//!   with a `NULL` argument is `NULL`. `LowCardinality` arguments are read as
//!   their values.
//! * DataFusion coerces nothing: every argument reaches the function with its
//!   own type.

use std::fmt;
use std::sync::Arc;

use chrono_tz::Tz;
use datafusion::arrow::array::{Array, ArrayRef, make_array, new_null_array};
use datafusion::arrow::buffer::NullBuffer;
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{DataType, Field, FieldRef};
use datafusion::common::{Result, ScalarValue};
use datafusion::logical_expr::simplify::{ExprSimplifyResult, SimplifyContext};
use datafusion::logical_expr::{
    ColumnarValue, Expr, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
};
use loams_house_sql::context::external;
use loams_house_types::arrow::{from_arrow, to_arrow};
use loams_house_types::{ChError, ChType};

use super::zone::{parse_zone, session_zone};

/// What a function's typing sees: its name, the arguments' ClickHouse types
/// (`Nullable` and `LowCardinality` taken off) and the constant arguments.
#[derive(Debug)]
pub struct Sig<'a> {
    /// The function's name.
    pub name: &'a str,
    /// The arguments' types.
    pub types: &'a [ChType],
    /// The arguments that are constants.
    pub consts: &'a [Option<ScalarValue>],
}

/// How a function types its result.
pub type TypeFn = fn(&Sig) -> Result<ChType, ChError>;

/// What a function's evaluation sees.
#[derive(Debug)]
pub struct Call<'a> {
    /// The function's name.
    pub name: &'a str,
    /// The arguments' types.
    pub types: &'a [ChType],
    /// The arguments that are constants.
    pub consts: &'a [Option<ScalarValue>],
    /// The arguments, one value per row.
    pub arrays: Vec<ArrayRef>,
    /// The result type (`Nullable` taken off).
    pub ret: ChType,
    /// The number of rows.
    pub rows: usize,
    /// The session's zone.
    pub session: Tz,
    /// The rows where every argument is non-`NULL`, if any is `NULL`.
    pub valid: Option<NullBuffer>,
}

/// How a function computes its result.
pub type EvalFn = fn(&Call) -> Result<ArrayRef, ChError>;

/// How a `Stable` function folds into a constant once per query (`now()`).
pub type FoldFn = fn(&SimplifyContext, &ChType, &[Expr]) -> Option<ScalarValue>;

impl Sig<'_> {
    /// Fails unless there are `min..=max` arguments.
    ///
    /// # Errors
    ///
    /// `42 NUMBER_OF_ARGUMENTS_DOESNT_MATCH`.
    pub fn arity(&self, min: usize, max: usize) -> Result<(), ChError> {
        arity(self.name, self.types.len(), min, max)
    }

    /// The constant string argument `i`, if there is one.
    ///
    /// # Errors
    ///
    /// `44 ILLEGAL_COLUMN` when argument `i` is there but not a constant
    /// string.
    pub fn const_str(&self, i: usize) -> Result<Option<String>, ChError> {
        const_str(self.name, self.types, self.consts, i)
    }

    /// The constant integer argument `i`, if there is one.
    ///
    /// # Errors
    ///
    /// `44` when argument `i` is there but not a constant integer.
    pub fn const_int(&self, i: usize) -> Result<Option<i64>, ChError> {
        const_int(self.name, self.types, self.consts, i)
    }

    /// The time zone argument `i` (a constant string), parsed.
    ///
    /// # Errors
    ///
    /// `44`, or `36` for an unknown zone.
    pub fn zone_arg(&self, i: usize) -> Result<Option<String>, ChError> {
        let z = self.const_str(i)?;
        if let Some(z) = &z {
            parse_zone(z)?;
        }
        Ok(z)
    }
}

impl Call<'_> {
    /// Whether row `row` has no `NULL` argument.
    pub fn valid(&self, row: usize) -> bool {
        self.valid.as_ref().is_none_or(|v| v.is_valid(row))
    }

    /// The constant string argument `i`, if there is one.
    ///
    /// # Errors
    ///
    /// As [`Sig::const_str`].
    pub fn const_str(&self, i: usize) -> Result<Option<String>, ChError> {
        const_str(self.name, self.types, self.consts, i)
    }

    /// The constant integer argument `i`, if there is one.
    ///
    /// # Errors
    ///
    /// As [`Sig::const_int`].
    pub fn const_int(&self, i: usize) -> Result<Option<i64>, ChError> {
        const_int(self.name, self.types, self.consts, i)
    }

    /// The zone of argument `i` (a constant), else `fallback`'s, else the
    /// session's.
    ///
    /// # Errors
    ///
    /// `44`, or `36` for an unknown zone.
    pub fn zone(&self, i: usize, fallback: Option<&str>) -> Result<Tz, ChError> {
        match self.const_str(i)? {
            Some(z) => parse_zone(&z),
            None => match fallback {
                Some(z) => parse_zone(z),
                None => Ok(self.session),
            },
        }
    }

    /// Maps every valid row through `f`, the others to `NULL`.
    ///
    /// # Errors
    ///
    /// The first error `f` returns.
    pub fn map<T>(
        &self,
        mut f: impl FnMut(usize) -> Result<Option<T>, ChError>,
    ) -> Result<Vec<Option<T>>, ChError> {
        (0..self.rows)
            .map(|row| if self.valid(row) { f(row) } else { Ok(None) })
            .collect()
    }
}

/// Fails unless `n` is in `min..=max`.
///
/// # Errors
///
/// `42 NUMBER_OF_ARGUMENTS_DOESNT_MATCH`.
pub fn arity(name: &str, n: usize, min: usize, max: usize) -> Result<(), ChError> {
    if n < min || n > max {
        let want = if min == max {
            min.to_string()
        } else {
            format!("{min} to {max}")
        };
        return Err(ChError::number_of_arguments_doesnt_match(format!(
            "Number of arguments for function {name} doesn't match: passed {n}, should be {want}"
        )));
    }
    Ok(())
}

fn scalar_str(v: &ScalarValue) -> Option<&str> {
    match v {
        ScalarValue::Utf8(s) | ScalarValue::LargeUtf8(s) | ScalarValue::Utf8View(s) => s.as_deref(),
        ScalarValue::Dictionary(_, inner) => scalar_str(inner),
        _ => None,
    }
}

fn scalar_int(v: &ScalarValue) -> Option<i64> {
    Some(match v {
        ScalarValue::Int8(Some(x)) => i64::from(*x),
        ScalarValue::Int16(Some(x)) => i64::from(*x),
        ScalarValue::Int32(Some(x)) => i64::from(*x),
        ScalarValue::Int64(Some(x)) => *x,
        ScalarValue::UInt8(Some(x)) => i64::from(*x),
        ScalarValue::UInt16(Some(x)) => i64::from(*x),
        ScalarValue::UInt32(Some(x)) => i64::from(*x),
        ScalarValue::UInt64(Some(x)) => *x as i64,
        ScalarValue::Dictionary(_, inner) => return scalar_int(inner),
        _ => return None,
    })
}

fn illegal_column(name: &str, i: usize, what: &str) -> ChError {
    ChError {
        code: 44,
        name: "ILLEGAL_COLUMN",
        message: format!(
            "Argument {} of function {name} must be a constant {what}",
            i + 1
        ),
    }
}

fn const_str(
    name: &str,
    types: &[ChType],
    consts: &[Option<ScalarValue>],
    i: usize,
) -> Result<Option<String>, ChError> {
    if i >= types.len() {
        return Ok(None);
    }
    consts
        .get(i)
        .and_then(Option::as_ref)
        .and_then(scalar_str)
        .map(|s| Some(s.to_string()))
        .ok_or_else(|| illegal_column(name, i, "string"))
}

fn const_int(
    name: &str,
    types: &[ChType],
    consts: &[Option<ScalarValue>],
    i: usize,
) -> Result<Option<i64>, ChError> {
    if i >= types.len() {
        return Ok(None);
    }
    consts
        .get(i)
        .and_then(Option::as_ref)
        .and_then(scalar_int)
        .map(Some)
        .ok_or_else(|| illegal_column(name, i, "integer"))
}

/// A ClickHouse scalar function. Two are equal when their names are: a name
/// is registered once.
pub struct ChFunction {
    name: String,
    signature: Signature,
    ty: TypeFn,
    eval: EvalFn,
    fold: Option<FoldFn>,
}

impl PartialEq for ChFunction {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.signature == other.signature
    }
}

impl Eq for ChFunction {}

impl std::hash::Hash for ChFunction {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.signature.hash(state);
    }
}

impl fmt::Debug for ChFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChFunction")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl ChFunction {
    /// An immutable function.
    pub fn new(name: &str, ty: TypeFn, eval: EvalFn) -> Self {
        Self {
            name: name.to_string(),
            signature: Signature::user_defined(Volatility::Immutable),
            ty,
            eval,
            fold: None,
        }
    }

    /// A function constant within a query (`now()`), folded by `fold`.
    pub fn stable(name: &str, ty: TypeFn, eval: EvalFn, fold: FoldFn) -> Self {
        Self {
            name: name.to_string(),
            signature: Signature::user_defined(Volatility::Stable),
            ty,
            eval,
            fold: Some(fold),
        }
    }

    fn arg_types(fields: &[FieldRef]) -> Result<Vec<ChType>, ChError> {
        fields
            .iter()
            .map(|f| from_arrow(f).map(|t| t.base().clone()))
            .collect()
    }

    /// The result type, `Nullable` as the arguments make it.
    fn result_type(
        &self,
        fields: &[FieldRef],
        consts: &[Option<ScalarValue>],
    ) -> Result<ChType, ChError> {
        let types = Self::arg_types(fields)?;
        if types.iter().any(|t| matches!(t, ChType::Nothing)) {
            return Ok(ChType::Nullable(Box::new(ChType::Nothing)));
        }
        let t = (self.ty)(&Sig {
            name: &self.name,
            types: &types,
            consts,
        })?;
        let nullable = fields.iter().any(|f| {
            from_arrow(f).is_ok_and(|t| t.is_nullable()) || f.data_type() == &DataType::Null
        });
        Ok(if nullable {
            loams_house_types::arrow::nullable(t)
        } else {
            t
        })
    }
}

fn unpack(a: ArrayRef) -> Result<ArrayRef, ChError> {
    match a.data_type() {
        DataType::Dictionary(_, values) => {
            cast(&a, values).map_err(|e| ChError::logical_error(e.to_string()))
        }
        _ => Ok(a),
    }
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
        Err(external(ChError::logical_error(format!(
            "{} is typed from its fields",
            self.name
        ))))
    }

    fn return_field_from_args(&self, args: ReturnFieldArgs) -> Result<FieldRef> {
        let consts: Vec<Option<ScalarValue>> =
            args.scalar_arguments.iter().map(|c| c.cloned()).collect();
        let t = self
            .result_type(args.arg_fields, &consts)
            .map_err(external)?;
        Ok(Arc::new(to_arrow(&self.name, &t).map_err(external)?))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let rows = args.number_rows;
        let ret = from_arrow(&args.return_field).map_err(external)?;
        let ret_base = ret.base().clone();
        if matches!(ret_base, ChType::Nothing) {
            return Ok(ColumnarValue::Array(new_null_array(&DataType::Null, rows)));
        }
        let types = Self::arg_types(&args.arg_fields).map_err(external)?;
        let consts: Vec<Option<ScalarValue>> = args
            .args
            .iter()
            .map(|a| match a {
                ColumnarValue::Scalar(s) => Some(s.clone()),
                ColumnarValue::Array(_) => None,
            })
            .collect();
        let arrays = args
            .args
            .iter()
            .map(|a| {
                a.to_array(rows)
                    .map_err(|e| ChError::logical_error(e.to_string()))
                    .and_then(unpack)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(external)?;
        let mut valid: Option<NullBuffer> = None;
        for a in &arrays {
            valid = NullBuffer::union(valid.as_ref(), a.logical_nulls().as_ref());
        }
        let call = Call {
            name: &self.name,
            types: &types,
            consts: &consts,
            arrays,
            ret: ret_base.clone(),
            rows,
            session: session_zone(&args.config_options),
            valid,
        };
        let out = (self.eval)(&call).map_err(external)?;
        let out = match &call.valid {
            None => out,
            Some(v) => {
                let nulls = NullBuffer::union(Some(v), out.logical_nulls().as_ref());
                let data = out.to_data().into_builder().nulls(nulls).build()?;
                make_array(data)
            }
        };
        // The field must be what the plan typed: `Nullable` or not, the
        // Arrow type exactly.
        let want = args.return_field.data_type();
        let out = if out.data_type() == want {
            out
        } else {
            cast(&out, want)?
        };
        Ok(ColumnarValue::Array(out))
    }

    fn simplify(&self, args: Vec<Expr>, info: &SimplifyContext) -> Result<ExprSimplifyResult> {
        let Some(fold) = self.fold else {
            return Ok(ExprSimplifyResult::Original(args));
        };
        // Constants only: a function of columns stays a call.
        let mut fields = Vec::with_capacity(args.len());
        let mut consts = Vec::with_capacity(args.len());
        for a in &args {
            match a {
                Expr::Literal(v, meta) => {
                    let mut f = Field::new("arg", v.data_type(), v.is_null());
                    if let Some(m) = meta {
                        f = f.with_metadata(m.to_hashmap());
                    }
                    fields.push(Arc::new(f));
                    consts.push(Some(v.clone()));
                }
                _ => return Ok(ExprSimplifyResult::Original(args)),
            }
        }
        let t = self.result_type(&fields, &consts).map_err(external)?;
        match fold(info, &t, &args) {
            Some(v) => {
                let field = to_arrow(&self.name, &t).map_err(external)?;
                Ok(ExprSimplifyResult::Simplified(Expr::Literal(
                    v,
                    Some(datafusion::common::metadata::FieldMetadata::from(&field)),
                )))
            }
            None => Ok(ExprSimplifyResult::Original(args)),
        }
    }
}

/// `ScalarUDF`s of `functions`, keyed by name.
pub fn udf(f: ChFunction) -> Arc<datafusion::logical_expr::ScalarUDF> {
    Arc::new(datafusion::logical_expr::ScalarUDF::new_from_impl(f))
}

/// The zone of a type's own, as the result type keeps it.
pub fn own_zone(t: &ChType) -> Option<String> {
    super::value::temporal(t).and_then(|(_, z)| z.map(str::to_string))
}

/// `43 ILLEGAL_TYPE_OF_ARGUMENT` naming argument `i` of `name`.
pub fn illegal(name: &str, t: &ChType, i: usize) -> ChError {
    ChError::illegal_type_of_argument(format!(
        "Illegal type {t} of argument {} of function {name}",
        i + 1
    ))
}

/// Whether the array is all `NULL`.
pub fn all_null(a: &dyn Array) -> bool {
    a.null_count() == a.len()
}
