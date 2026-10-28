//! ClickHouse's higher-order functions on the Rust engine (plan HE1 Task 32),
//! as DataFusion `HigherOrderUDF`s in `Registry.higher_order`: `arrayMap`,
//! `arrayFilter`, `arrayExists`, `arrayAll`, `arrayCount`, `arrayFirst`,
//! `arrayFirstIndex`, `arrayLast`, `arrayLastIndex`, `arraySum`, `arraySort`,
//! `arrayReverseSort` and `mapFilter` (ClickHouse
//! `src/Functions/array/FunctionArrayMapped.h` and the `array*.cpp` beside it,
//! `src/Functions/map.cpp`'s `FunctionMapToArrayAdapter`, Apache-2.0).
//!
//! **ClickHouse's form.** The lambda comes first and takes one parameter per
//! array: `arrayMap((x, y) -> x + y, a, b)`. The arrays must be the same
//! length on each row (`190`). A condition is true when it is neither zero nor
//! `NULL`.
//!
//! **Without a lambda.** `arrayExists(a)`, `arrayAll(a)`, `arrayCount(a)`,
//! `arraySum(a)`, `arraySort(a)` and `arrayReverseSort(a)` read the elements
//! themselves, so these names are higher-order functions only: DataFusion
//! looks a scalar function up first, and a scalar of the same name would
//! never see the lambda.
//!
//! **`mapFilter`.** ClickHouse passes a two-parameter lambda the key and the
//! value, and a one-parameter lambda the entry as a `Tuple(K, V)` (Langfuse's
//! `mapFilter(x -> … x.1 …, m)`). DataFusion does not tell a function how
//! many parameters its lambda declares, so the guard renames the
//! one-parameter form to [`MAP_FILTER_TUPLE`].

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, UInt8Array, UInt32Array};
use datafusion::arrow::compute::take_arrays;
use datafusion::arrow::datatypes::{DataType, Field, FieldRef};
use datafusion::common::{DataFusionError, Result};
use datafusion::logical_expr::{
    ColumnarValue, HigherOrderFunctionArgs, HigherOrderReturnFieldArgs, HigherOrderSignature,
    HigherOrderUDF, HigherOrderUDFImpl, LambdaArgument, LambdaParametersProgress, ValueOrLambda,
    Volatility,
};
use loams_house_sql::Registry;
use loams_house_types::arrow::{CH_TYPE, data_type, from_arrow, to_arrow};
use loams_house_types::{ChError, ChType};

use crate::func::arrays::fold::{sort_order, sum, sum_type};
use crate::func::maps::build_map;
use crate::func::nested::values::{
    ListParts, build_list, gather_list, take_indexes, truths, with_default,
};
use crate::func::nested::{arity, coerce_to, df, illegal_type, sizes_dont_match};

/// The name the guard gives `mapFilter` called with a one-parameter lambda.
pub const MAP_FILTER_TUPLE: &str = "__ch_mapFilter_tuple";

/// What a higher-order function does with its lambda's results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// `arrayMap`: the results, as an array.
    Map,
    /// `arrayFilter`: the first array's elements where the result is true.
    Filter,
    /// `arrayExists`: whether any result is true.
    Exists,
    /// `arrayAll`: whether every result is true.
    All,
    /// `arrayCount`: how many results are true.
    Count,
    /// `arrayFirst`: the first array's first element whose result is true,
    /// else the element type's default.
    First,
    /// `arrayFirstIndex`: its position from 1, else 0.
    FirstIndex,
    /// `arrayLast`: the last such element.
    Last,
    /// `arrayLastIndex`: its position from 1, else 0.
    LastIndex,
    /// `arraySum`: the sum of the results.
    Sum,
    /// `arraySort`: the first array ordered by the results.
    Sort,
    /// `arrayReverseSort`: ordered by the results, descending.
    ReverseSort,
    /// `mapFilter((k, v) -> …, m)`.
    MapFilter,
    /// `mapFilter(kv -> …, m)`, the entry as a tuple.
    MapFilterTuple,
}

impl Kind {
    fn needs_lambda(self) -> bool {
        !matches!(
            self,
            Kind::Exists | Kind::All | Kind::Count | Kind::Sum | Kind::Sort | Kind::ReverseSort
        )
    }

    fn is_map(self) -> bool {
        matches!(self, Kind::MapFilter | Kind::MapFilterTuple)
    }

    fn is_condition(self) -> bool {
        matches!(
            self,
            Kind::Filter
                | Kind::Exists
                | Kind::All
                | Kind::Count
                | Kind::First
                | Kind::FirstIndex
                | Kind::Last
                | Kind::LastIndex
                | Kind::MapFilter
                | Kind::MapFilterTuple
        )
    }
}

/// Every higher-order function: its registered name, its ClickHouse name and
/// its kind.
pub const FUNCTIONS: &[(&str, &str, Kind)] = &[
    ("arrayMap", "arrayMap", Kind::Map),
    ("arrayFilter", "arrayFilter", Kind::Filter),
    ("arrayExists", "arrayExists", Kind::Exists),
    ("arrayAll", "arrayAll", Kind::All),
    ("arrayCount", "arrayCount", Kind::Count),
    ("arrayFirst", "arrayFirst", Kind::First),
    ("arrayFirstIndex", "arrayFirstIndex", Kind::FirstIndex),
    ("arrayLast", "arrayLast", Kind::Last),
    ("arrayLastIndex", "arrayLastIndex", Kind::LastIndex),
    ("arraySum", "arraySum", Kind::Sum),
    ("arraySort", "arraySort", Kind::Sort),
    ("arrayReverseSort", "arrayReverseSort", Kind::ReverseSort),
    ("mapFilter", "mapFilter", Kind::MapFilter),
    (MAP_FILTER_TUPLE, "mapFilter", Kind::MapFilterTuple),
];

/// The canonical names of the functions this module registers.
pub const NAMES: &[&str] = &[
    "arrayMap",
    "arrayFilter",
    "arrayExists",
    "arrayAll",
    "arrayCount",
    "arrayFirst",
    "arrayFirstIndex",
    "arrayLast",
    "arrayLastIndex",
    "arraySum",
    "arraySort",
    "arrayReverseSort",
    "mapFilter",
];

/// Registers the higher-order functions.
pub fn register(r: &mut Registry) {
    for (internal, name, kind) in FUNCTIONS {
        r.higher_order.insert(
            (*internal).to_string(),
            Arc::new(HigherOrderUDF::new_from_impl(ArrayHof::new(
                internal, name, *kind,
            ))),
        );
    }
}

/// A ClickHouse higher-order function.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct ArrayHof {
    internal: &'static str,
    ch_name: &'static str,
    kind: Kind,
    signature: HigherOrderSignature,
}

impl ArrayHof {
    /// The function `name` (registered as `internal`) of `kind`.
    pub fn new(internal: &'static str, name: &'static str, kind: Kind) -> Self {
        Self {
            internal,
            ch_name: name,
            kind,
            signature: HigherOrderSignature::user_defined(Volatility::Immutable),
        }
    }

    /// Its result type, from its value arguments' types and its lambda's
    /// (`None` without one).
    pub fn result_type(
        &self,
        values: &[ChType],
        lambda: Option<&ChType>,
    ) -> Result<ChType, ChError> {
        let name = self.ch_name;
        if values.is_empty() {
            return Err(arity(name, 0, "at least 1"));
        }
        if lambda.is_none() {
            if self.kind.needs_lambda() {
                return Err(ChError::number_of_arguments_doesnt_match(format!(
                    "Function {name} needs a lambda function as its first argument"
                )));
            }
            if values.len() != 1 {
                return Err(arity(name, values.len(), "1 without a lambda function"));
            }
        }
        if self.kind.is_map() {
            if values.len() != 1 {
                return Err(arity(name, values.len() + 1, "2"));
            }
            if !matches!(values[0], ChType::Map(..)) {
                return Err(illegal_type(name, 1, &values[0]));
            }
        } else {
            for (i, t) in values.iter().enumerate() {
                if !matches!(t, ChType::Array(_)) {
                    return Err(illegal_type(name, i + usize::from(lambda.is_some()), t));
                }
            }
        }
        let first_elem = match &values[0] {
            ChType::Array(e) => Some((**e).clone()),
            _ => None,
        };
        let produced = match lambda {
            Some(t) => t.clone(),
            None => first_elem.clone().unwrap_or(ChType::Nothing),
        };
        if self.kind.is_condition() && !is_condition_type(&produced) {
            return Err(ChError::illegal_type_of_argument(format!(
                "Expression for function {name} must return UInt8 or Nullable(UInt8), returns {produced}"
            )));
        }
        Ok(match self.kind {
            Kind::Map => ChType::Array(Box::new(produced)),
            Kind::Filter
            | Kind::Sort
            | Kind::ReverseSort
            | Kind::MapFilter
            | Kind::MapFilterTuple => values[0].clone(),
            Kind::Exists | Kind::All => ChType::UInt8,
            Kind::Count | Kind::FirstIndex | Kind::LastIndex => ChType::UInt32,
            Kind::First | Kind::Last => first_elem.unwrap_or(ChType::Nothing),
            Kind::Sum => sum_type(name, &produced)?,
        })
    }

    /// The lambda's parameters for value arguments `values`.
    fn parameters(&self, values: &[FieldRef]) -> Result<Vec<FieldRef>, ChError> {
        let entries = |f: &FieldRef| -> Result<FieldRef, ChError> {
            match f.data_type() {
                DataType::Map(entries, _) => Ok(Arc::clone(entries)),
                _ => Err(illegal_type(self.ch_name, 1, &from_arrow(f)?)),
            }
        };
        match self.kind {
            Kind::MapFilter => {
                let e = entries(values.first().ok_or_else(|| arity(self.ch_name, 1, "2"))?)?;
                let DataType::Struct(kv) = e.data_type() else {
                    return Err(ChError::logical_error("Map entries are not a Struct"));
                };
                Ok(vec![Arc::clone(&kv[0]), Arc::clone(&kv[1])])
            }
            Kind::MapFilterTuple => {
                let f = values.first().ok_or_else(|| arity(self.ch_name, 1, "2"))?;
                let e = entries(f)?;
                let ChType::Map(k, v) = from_arrow(f)? else {
                    return Err(illegal_type(self.ch_name, 1, &from_arrow(f)?));
                };
                let tuple = ChType::Tuple(vec![(None, *k), (None, *v)]);
                let mut metadata = e.metadata().clone();
                metadata.insert(CH_TYPE.to_string(), tuple.to_compact_string());
                Ok(vec![Arc::new(e.as_ref().clone().with_metadata(metadata))])
            }
            _ => values
                .iter()
                .enumerate()
                .map(|(i, f)| match f.data_type() {
                    DataType::List(item) | DataType::LargeList(item) => Ok(Arc::clone(item)),
                    _ => Err(illegal_type(self.ch_name, i + 1, &from_arrow(f)?)),
                })
                .collect(),
        }
    }
}

/// Whether a lambda's (or an element's) type is a condition: an integer or
/// `Bool`, possibly `Nullable` (`Nothing` for an always-`NULL` condition).
fn is_condition_type(t: &ChType) -> bool {
    matches!(
        t.base(),
        ChType::UInt8
            | ChType::UInt16
            | ChType::UInt32
            | ChType::UInt64
            | ChType::Int8
            | ChType::Int16
            | ChType::Int32
            | ChType::Int64
            | ChType::Bool
            | ChType::Nothing
    )
}

impl HigherOrderUDFImpl for ArrayHof {
    fn name(&self) -> &str {
        self.internal
    }

    fn signature(&self) -> &HigherOrderSignature {
        &self.signature
    }

    fn coerce_value_types(&self, arg_types: &[DataType]) -> Result<Vec<DataType>> {
        Ok(arg_types.to_vec())
    }

    fn lambda_parameters(
        &self,
        _step: usize,
        fields: &[ValueOrLambda<FieldRef, Option<FieldRef>>],
    ) -> Result<LambdaParametersProgress> {
        let lambdas = fields
            .iter()
            .filter(|f| matches!(f, ValueOrLambda::Lambda(_)))
            .count();
        if lambdas == 0 {
            return Ok(LambdaParametersProgress::Complete(vec![]));
        }
        if lambdas > 1 {
            return Err(df(ChError::number_of_arguments_doesnt_match(format!(
                "Function {} takes one lambda function, got {lambdas}",
                self.ch_name
            ))));
        }
        let values: Vec<FieldRef> = fields
            .iter()
            .filter_map(|f| match f {
                ValueOrLambda::Value(v) => Some(Arc::clone(v)),
                ValueOrLambda::Lambda(_) => None,
            })
            .collect();
        let params = self.parameters(&values).map_err(df)?;
        Ok(LambdaParametersProgress::Complete(vec![params]))
    }

    fn return_field_from_args(&self, args: HigherOrderReturnFieldArgs) -> Result<FieldRef> {
        let mut values = Vec::new();
        let mut lambda = None;
        for a in args.arg_fields {
            match a {
                ValueOrLambda::Value(f) => values.push(from_arrow(f).map_err(df)?),
                ValueOrLambda::Lambda(f) => lambda = Some(from_arrow(f).map_err(df)?),
            }
        }
        let t = self.result_type(&values, lambda.as_ref()).map_err(df)?;
        Ok(Arc::new(to_arrow(self.ch_name, &t).map_err(df)?))
    }

    fn invoke_with_args(&self, args: HigherOrderFunctionArgs) -> Result<ColumnarValue> {
        let rows = args.number_rows;
        let mut values = Vec::new();
        let mut types = Vec::new();
        let mut lambda: Option<&LambdaArgument> = None;
        let mut lambda_type: Option<ChType> = None;
        for (a, f) in args.args.iter().zip(&args.arg_fields) {
            match (a, f) {
                (ValueOrLambda::Value(v), ValueOrLambda::Value(field)) => {
                    values.push(v.to_array(rows)?);
                    types.push(from_arrow(field).map_err(df)?);
                }
                (ValueOrLambda::Lambda(l), field) => {
                    lambda = Some(l);
                    if let ValueOrLambda::Lambda(field) = field {
                        lambda_type = Some(from_arrow(field).map_err(df)?);
                    }
                }
                (ValueOrLambda::Value(_), ValueOrLambda::Lambda(_)) => {
                    return Err(DataFusionError::Internal(format!(
                        "{}: a value argument has a lambda's field",
                        self.ch_name
                    )));
                }
            }
        }
        let out = self
            .run(
                rows,
                &values,
                &types,
                lambda.map(|l| (l, lambda_type)),
                &args.return_field,
            )
            .map_err(|e| match e {
                Failure::Ch(e) => df(e),
                Failure::Df(e) => e,
            })?;
        let out = coerce_to(&out, args.return_field.data_type()).map_err(df)?;
        Ok(ColumnarValue::Array(out))
    }
}

/// An error from the lambda (DataFusion's) or from the function (ClickHouse's).
enum Failure {
    Ch(ChError),
    Df(DataFusionError),
}

impl From<ChError> for Failure {
    fn from(e: ChError) -> Self {
        Failure::Ch(e)
    }
}

impl From<DataFusionError> for Failure {
    fn from(e: DataFusionError) -> Self {
        Failure::Df(e)
    }
}

impl ArrayHof {
    fn run(
        &self,
        rows: usize,
        values: &[ArrayRef],
        types: &[ChType],
        lambda: Option<(&LambdaArgument, Option<ChType>)>,
        return_field: &Field,
    ) -> std::result::Result<ArrayRef, Failure> {
        let ret = from_arrow(return_field)?;
        let ret_type = data_type(&ret)?.0;
        let parts = values
            .iter()
            .map(ListParts::of)
            .collect::<Result<Vec<_>, _>>()?;
        for r in 0..rows {
            let n = parts[0].len(r);
            if parts.iter().any(|p| p.len(r) != n) {
                return Err(sizes_dont_match(self.ch_name).into());
            }
        }
        let base = parts[0].offsets[0];
        let rel: Vec<usize> = parts[0].offsets[..=rows].iter().map(|o| o - base).collect();
        let used: Vec<ArrayRef> = parts.iter().map(ListParts::used_values).collect();
        let elements = used[0].len();

        // The lambda's arguments, one array per parameter.
        let params: Vec<ArrayRef> = match self.kind {
            Kind::MapFilter => {
                let e = used[0].as_struct();
                vec![Arc::clone(e.column(0)), Arc::clone(e.column(1))]
            }
            _ => used.clone(),
        };
        let results: Option<ArrayRef> = match lambda {
            Some((l, _)) => {
                let fns: Vec<Box<dyn Fn() -> Result<ArrayRef>>> = params
                    .iter()
                    .map(|p| {
                        let p = Arc::clone(p);
                        Box::new(move || Ok(Arc::clone(&p))) as Box<dyn Fn() -> Result<ArrayRef>>
                    })
                    .collect();
                let refs: Vec<&dyn Fn() -> Result<ArrayRef>> =
                    fns.iter().map(|f| f.as_ref()).collect();
                let owner = parts[0].row_of_each_element();
                let out = l.evaluate(&refs, |captured| Ok(take_arrays(captured, &owner, None)?))?;
                Some(out.into_array(elements)?)
            }
            None => None,
        };
        let produced = results.clone().unwrap_or_else(|| Arc::clone(&used[0]));
        let row_ranges = |r: usize| rel[r]..rel[r + 1];

        Ok(match self.kind {
            Kind::Map => build_list(&ret_type, (0..rows).map(|r| rel[r + 1] - rel[r]), produced)?,
            Kind::Filter => {
                let t = truths(&produced)?;
                let picks: Vec<Vec<u64>> = (0..rows)
                    .map(|r| row_ranges(r).filter(|j| t[*j]).map(|j| j as u64).collect())
                    .collect();
                gather_list(&ret_type, &used[0], &picks)?
            }
            Kind::Exists | Kind::All => {
                let t = truths(&produced)?;
                Arc::new(UInt8Array::from(
                    (0..rows)
                        .map(|r| {
                            let mut it = row_ranges(r).map(|j| t[j]);
                            u8::from(if self.kind == Kind::All {
                                it.all(|b| b)
                            } else {
                                it.any(|b| b)
                            })
                        })
                        .collect::<Vec<_>>(),
                ))
            }
            Kind::Count => {
                let t = truths(&produced)?;
                Arc::new(UInt32Array::from(
                    (0..rows)
                        .map(|r| row_ranges(r).filter(|j| t[*j]).count() as u32)
                        .collect::<Vec<_>>(),
                ))
            }
            Kind::First | Kind::Last | Kind::FirstIndex | Kind::LastIndex => {
                let t = truths(&produced)?;
                let found: Vec<Option<usize>> = (0..rows)
                    .map(|r| {
                        let mut range = row_ranges(r);
                        if matches!(self.kind, Kind::First | Kind::FirstIndex) {
                            range.find(|j| t[*j])
                        } else {
                            range.rev().find(|j| t[*j])
                        }
                    })
                    .collect();
                if matches!(self.kind, Kind::FirstIndex | Kind::LastIndex) {
                    Arc::new(UInt32Array::from(
                        found
                            .iter()
                            .enumerate()
                            .map(|(r, f)| f.map_or(0, |j| (j - rel[r] + 1) as u32))
                            .collect::<Vec<_>>(),
                    ))
                } else {
                    let ChType::Array(elem) = &types[0] else {
                        return Err(illegal_type(self.ch_name, 1, &types[0]).into());
                    };
                    let source = with_default(&used[0], elem)?;
                    let picks: Vec<u64> = found
                        .iter()
                        .map(|f| f.map_or(elements as u64, |j| j as u64))
                        .collect();
                    take_indexes(&source, &picks)?
                }
            }
            Kind::Sum => {
                let t = match lambda {
                    Some((_, Some(t))) => t.clone(),
                    Some((_, None)) => from_lambda_type(&produced),
                    None => match &types[0] {
                        ChType::Array(e) => (**e).clone(),
                        other => return Err(illegal_type(self.ch_name, 0, other).into()),
                    },
                };
                sum(self.ch_name, &produced, &t, &rel)?
            }
            Kind::Sort | Kind::ReverseSort => {
                let order = sort_order(&produced, &rel, self.kind == Kind::ReverseSort)?;
                let picks: Vec<Vec<u64>> = order
                    .into_iter()
                    .map(|o| o.into_iter().map(|j| j as u64).collect())
                    .collect();
                gather_list(&ret_type, &used[0], &picks)?
            }
            Kind::MapFilter | Kind::MapFilterTuple => {
                let t = truths(&produced)?;
                let mut lengths = Vec::with_capacity(rows);
                let mut picks = Vec::new();
                for r in 0..rows {
                    let before = picks.len();
                    picks.extend(row_ranges(r).filter(|j| t[*j]).map(|j| j as u64));
                    lengths.push(picks.len() - before);
                }
                let entries = used[0].as_struct();
                build_map(
                    &ret_type,
                    &lengths,
                    take_indexes(entries.column(0), &picks)?,
                    take_indexes(entries.column(1), &picks)?,
                )?
            }
        })
    }
}

/// The ClickHouse type of a lambda's results, from the array itself (its
/// field metadata is not at hand here): inferred from Arrow.
fn from_lambda_type(a: &ArrayRef) -> ChType {
    let field = Field::new("", a.data_type().clone(), false);
    from_arrow(&field).unwrap_or(ChType::Nothing)
}

/// Every registered name, for the registry tests.
pub fn registered_names() -> HashMap<&'static str, Kind> {
    FUNCTIONS.iter().map(|(i, _, k)| (*i, *k)).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn arr(t: ChType) -> ChType {
        ChType::Array(Box::new(t))
    }

    #[test]
    fn result_types_follow_clickhouse() {
        let f = |k| ArrayHof::new("f", "f", k);
        let s = arr(ChType::String);
        assert_eq!(
            f(Kind::Map)
                .result_type(&[s.clone()], Some(&ChType::UInt64))
                .unwrap(),
            arr(ChType::UInt64)
        );
        assert_eq!(
            f(Kind::Filter)
                .result_type(&[s.clone()], Some(&ChType::UInt8))
                .unwrap(),
            s
        );
        assert_eq!(
            f(Kind::FirstIndex)
                .result_type(&[s.clone()], Some(&ChType::UInt8))
                .unwrap(),
            ChType::UInt32
        );
        assert_eq!(
            f(Kind::First)
                .result_type(&[s.clone()], Some(&ChType::Bool))
                .unwrap(),
            ChType::String
        );
        assert_eq!(
            f(Kind::Sum)
                .result_type(&[arr(ChType::Int8)], None)
                .unwrap(),
            ChType::Int64
        );
        assert_eq!(
            f(Kind::Filter)
                .result_type(&[s.clone()], Some(&ChType::String))
                .unwrap_err()
                .code,
            43
        );
        assert_eq!(
            f(Kind::Map)
                .result_type(&[s.clone()], None)
                .unwrap_err()
                .code,
            42
        );
        assert_eq!(
            f(Kind::Count)
                .result_type(&[arr(ChType::UInt8)], None)
                .unwrap(),
            ChType::UInt32
        );
    }

    #[test]
    fn map_filter_takes_a_map() {
        let m = ChType::Map(Box::new(ChType::String), Box::new(ChType::UInt64));
        let f = ArrayHof::new("mapFilter", "mapFilter", Kind::MapFilter);
        assert_eq!(
            f.result_type(&[m.clone()], Some(&ChType::UInt8)).unwrap(),
            m
        );
        let field = Arc::new(to_arrow("m", &m).unwrap());
        assert_eq!(f.parameters(&[field.clone()]).unwrap().len(), 2);
        let t = ArrayHof::new(MAP_FILTER_TUPLE, "mapFilter", Kind::MapFilterTuple);
        let p = t.parameters(&[field]).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(
            from_arrow(&p[0]).unwrap(),
            ChType::Tuple(vec![(None, ChType::String), (None, ChType::UInt64)])
        );
    }
}
