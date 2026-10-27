//! The combinators: `Null` (implicit), `-If`, `-Array`, `-ForEach`,
//! `-Distinct`, `-OrNull`, `-OrDefault`, `-SimpleState` and `-Map`, each a
//! wrapper over the [`Agg`] built for the argument types it hands inside
//! (ClickHouse's `src/AggregateFunctions/Combinators/*`, Apache-2.0).

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, MapArray, StructArray, UInt8Array, UInt32Array,
    new_empty_array,
};
use datafusion::arrow::buffer::OffsetBuffer;
use datafusion::arrow::compute::kernels::cmp::neq;
use datafusion::arrow::compute::{and, concat, filter, is_not_null, take};
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::row::{OwnedRow, RowConverter, SortField};
use datafusion::common::{Result, ScalarValue};
use datafusion::logical_expr::Accumulator;
use loams_house_sql::aggregates::{AggregateName, Combinator};
use loams_house_types::ty::AggregateSpec;
use loams_house_types::{ChError, ChType};

use super::registry::{build, build_nested, illegal_type};
use super::{
    Agg, AggRef, arrow_type, can_be_inside_nullable, ch, conform, decode, default_scalar,
    item_field, list_scalar, null_scalar, state_struct_list, state_struct_list_type,
    state_struct_rows,
};

/// `190 SIZES_OF_ARRAYS_DONT_MATCH`.
fn sizes_differ(function: &str) -> ChError {
    ChError {
        code: 190,
        name: "SIZES_OF_ARRAYS_DONT_MATCH",
        message: format!(
            "Lengths of all arrays passed to aggregate function {function} must be equal"
        ),
    }
}

/// The `Null` combinator (`AggregateFunctionNull.cpp`): the function over the
/// arguments' non-`Nullable` types, skipping every row with a `NULL` in a
/// `Nullable` argument; `Nullable` when the nested result can be and the
/// function does not return its default on `NULL`-only input. Under `-If`,
/// the condition's nullability does not make the result `Nullable`
/// (`AggregateFunctionIf::getOwnNullAdapter`).
pub fn null(name: &AggregateName, args: &[ChType]) -> Result<AggRef, ChError> {
    // `-SimpleState` takes `Null` inside it (`tryTransformStateFunction`).
    if name.combinators.first() == Some(&Combinator::SimpleState) {
        return build_nested(name, args);
    }
    if args
        .iter()
        .any(|t| matches!(t.strip_nullable(), ChType::Nothing))
    {
        // `count(NULL)` is 0; any other function of a `NULL` literal is `NULL`.
        let count = name.base == "count" && name.combinators.is_empty();
        return Ok(Arc::new(Nothing {
            result: if count {
                ChType::UInt64
            } else {
                ChType::Nullable(Box::new(ChType::Nothing))
            },
        }));
    }
    let stripped: Vec<ChType> = args.iter().map(|t| t.strip_nullable().clone()).collect();
    let nested = build_nested(name, &stripped)?;
    let considered = if name.combinators.first() == Some(&Combinator::If) && !args.is_empty() {
        &args[..args.len() - 1]
    } else {
        args
    };
    let nullable_result = considered.iter().any(ChType::is_nullable)
        && !nested.default_when_only_null()
        && can_be_inside_nullable(nested.result());
    let result = if nullable_result {
        ChType::Nullable(Box::new(nested.result().clone()))
    } else {
        nested.result().clone()
    };
    Ok(Arc::new(NullWrap {
        mask: args.iter().map(ChType::is_nullable).collect(),
        nullable_result,
        result,
        nested,
    }))
}

/// A function of a `NULL` literal.
#[derive(Debug)]
struct Nothing {
    result: ChType,
}

impl Agg for Nothing {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        vec![DataType::Boolean]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        Ok(Box::new(NothingAcc(self.result.clone())))
    }

    fn default_when_only_null(&self) -> bool {
        self.result == ChType::UInt64
    }
}

#[derive(Debug)]
struct NothingAcc(ChType);

impl Accumulator for NothingAcc {
    fn update_batch(&mut self, _values: &[ArrayRef]) -> Result<()> {
        Ok(())
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        if self.0 == ChType::UInt64 {
            Ok(ScalarValue::UInt64(Some(0)))
        } else {
            Ok(ScalarValue::Null)
        }
    }

    fn size(&self) -> usize {
        size_of::<Self>()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        Ok(vec![ScalarValue::Boolean(Some(false))])
    }

    fn merge_batch(&mut self, _states: &[ArrayRef]) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct NullWrap {
    mask: Vec<bool>,
    nullable_result: bool,
    result: ChType,
    nested: AggRef,
}

impl Agg for NullWrap {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        let mut s = vec![DataType::Boolean];
        s.extend(self.nested.state());
        s
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        Ok(Box::new(NullAcc {
            mask: self.mask.clone(),
            nullable_result: self.nullable_result,
            result: self.result.clone(),
            seen: false,
            nested: self.nested.accumulator()?,
        }))
    }

    fn default_when_only_null(&self) -> bool {
        self.nested.default_when_only_null()
    }
}

#[derive(Debug)]
struct NullAcc {
    mask: Vec<bool>,
    nullable_result: bool,
    result: ChType,
    seen: bool,
    nested: Box<dyn Accumulator>,
}

/// `values` filtered to the rows where `keep` is true.
fn filter_all(values: &[ArrayRef], keep: &BooleanArray) -> Result<Vec<ArrayRef>> {
    values
        .iter()
        .map(|a| Ok(filter(a.as_ref(), keep)?))
        .collect()
}

impl Accumulator for NullAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        let mut keep: Option<BooleanArray> = None;
        for (a, nullable) in values.iter().zip(&self.mask) {
            if *nullable && a.null_count() > 0 {
                let valid = is_not_null(a.as_ref())?;
                keep = Some(match keep {
                    None => valid,
                    Some(k) => and(&k, &valid)?,
                });
            }
        }
        let values = match keep {
            Some(k) => filter_all(values, &k)?,
            None => values.to_vec(),
        };
        if values.first().is_some_and(|a| !a.is_empty()) {
            self.seen = true;
            self.nested.update_batch(&values)?;
        }
        Ok(())
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        if self.nullable_result && !self.seen {
            return null_scalar(&self.result);
        }
        self.nested.evaluate()
    }

    fn size(&self) -> usize {
        size_of::<Self>() + self.nested.size()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        let mut s = vec![ScalarValue::Boolean(Some(self.seen))];
        s.extend(self.nested.state()?);
        Ok(s)
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        if states[0].as_boolean().iter().any(|b| b == Some(true)) {
            self.seen = true;
        }
        self.nested.merge_batch(&states[1..])
    }
}

/// `-If` below the outermost combinator (the outermost one is DataFusion's
/// `FILTER`): the last argument is a `UInt8` or `Bool` condition.
pub fn if_(inner: &AggregateName, args: &[ChType], function: &str) -> Result<AggRef, ChError> {
    let Some((cond, rest)) = args.split_last() else {
        return Err(super::registry::arity(function, "at least one argument", 0));
    };
    if !matches!(cond, ChType::UInt8 | ChType::Bool) {
        return Err(ChError::illegal_type_of_argument(format!(
            "Illegal type {cond} of last argument for aggregate function with If suffix"
        )));
    }
    Ok(Arc::new(IfWrap {
        nested: build(inner, rest)?,
    }))
}

#[derive(Debug)]
struct IfWrap {
    nested: AggRef,
}

impl Agg for IfWrap {
    fn result(&self) -> &ChType {
        self.nested.result()
    }

    fn state(&self) -> Vec<DataType> {
        self.nested.state()
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        Ok(Box::new(IfAcc {
            nested: self.nested.accumulator()?,
        }))
    }

    fn default_when_only_null(&self) -> bool {
        self.nested.default_when_only_null()
    }
}

#[derive(Debug)]
struct IfAcc {
    nested: Box<dyn Accumulator>,
}

/// A condition column as a filter: non-zero and not `NULL`.
pub fn condition(cond: &ArrayRef) -> Result<BooleanArray> {
    Ok(match cond.data_type() {
        DataType::Boolean => cond.as_boolean().clone(),
        _ => {
            let c = conform(cond.clone(), &DataType::UInt8)?;
            neq(&c, &UInt8Array::new_scalar(0))?
        }
    })
}

impl Accumulator for IfAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        let Some((cond, rest)) = values.split_last() else {
            return Ok(());
        };
        let keep = condition(cond)?;
        let rest = filter_all(rest, &keep)?;
        if rest.first().is_some_and(|a| !a.is_empty()) {
            self.nested.update_batch(&rest)?;
        }
        Ok(())
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        self.nested.evaluate()
    }

    fn size(&self) -> usize {
        size_of::<Self>() + self.nested.size()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        self.nested.state()
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        self.nested.merge_batch(states)
    }
}

fn elements(args: &[ChType], function: &str) -> Result<Vec<ChType>, ChError> {
    if args.is_empty() {
        return Err(super::registry::arity(function, "at least one argument", 0));
    }
    args.iter()
        .map(|t| match t {
            ChType::Array(inner) => Ok((**inner).clone()),
            other => Err(ChError::illegal_type_of_argument(format!(
                "Illegal type {other} of argument for aggregate function {function}, must be array"
            ))),
        })
        .collect()
}

/// `-Array` (`AggregateFunctionArray.h`): the arguments are arrays of equal
/// length per row, aggregated element by element.
pub fn array(inner: &AggregateName, args: &[ChType], function: &str) -> Result<AggRef, ChError> {
    let elems = elements(args, function)?;
    Ok(Arc::new(ArrayWrap {
        nested: build(inner, &elems)?,
        function: function.to_string(),
    }))
}

#[derive(Debug)]
struct ArrayWrap {
    nested: AggRef,
    function: String,
}

impl Agg for ArrayWrap {
    fn result(&self) -> &ChType {
        self.nested.result()
    }

    fn state(&self) -> Vec<DataType> {
        self.nested.state()
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        Ok(Box::new(ArrayAcc {
            nested: self.nested.accumulator()?,
            function: self.function.clone(),
        }))
    }

    fn default_when_only_null(&self) -> bool {
        self.nested.default_when_only_null()
    }
}

#[derive(Debug)]
struct ArrayAcc {
    nested: Box<dyn Accumulator>,
    function: String,
}

/// Every row's element lengths of the list arrays `values`, checked equal.
fn lengths(values: &[ArrayRef], function: &str) -> Result<Vec<usize>> {
    let first = values[0].as_list::<i32>();
    let lens: Vec<usize> = first
        .offsets()
        .windows(2)
        .map(|w| (w[1] - w[0]) as usize)
        .collect();
    for a in &values[1..] {
        let l = a.as_list::<i32>();
        if l.offsets()
            .windows(2)
            .map(|w| (w[1] - w[0]) as usize)
            .ne(lens.iter().copied())
        {
            return Err(ch(sizes_differ(function)));
        }
    }
    Ok(lens)
}

/// The elements of every row of the list array `a`.
fn flat(a: &ArrayRef) -> ArrayRef {
    let l = a.as_list::<i32>();
    let offsets = l.offsets();
    let start = offsets[0] as usize;
    let end = offsets[offsets.len() - 1] as usize;
    l.values().slice(start, end - start)
}

impl Accumulator for ArrayAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        lengths(values, &self.function)?;
        let flat: Vec<ArrayRef> = values.iter().map(flat).collect();
        let flat = decode(&flat)?;
        if flat.first().is_some_and(|a| !a.is_empty()) {
            self.nested.update_batch(&flat)?;
        }
        Ok(())
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        self.nested.evaluate()
    }

    fn size(&self) -> usize {
        size_of::<Self>() + self.nested.size()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        self.nested.state()
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        self.nested.merge_batch(states)
    }
}

/// `-ForEach` (`AggregateFunctionForEach.h`): one aggregate per array index;
/// an `Array` of their results.
pub fn for_each(inner: &AggregateName, args: &[ChType], function: &str) -> Result<AggRef, ChError> {
    let elems = elements(args, function)?;
    let nested = build(inner, &elems)?;
    Ok(Arc::new(ForEachWrap {
        result: ChType::Array(Box::new(nested.result().clone())),
        nested,
        function: function.to_string(),
    }))
}

#[derive(Debug)]
struct ForEachWrap {
    result: ChType,
    nested: AggRef,
    function: String,
}

impl Agg for ForEachWrap {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        vec![state_struct_list_type(&self.nested.state())]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        Ok(Box::new(ForEachAcc {
            nested: self.nested.clone(),
            function: self.function.clone(),
            accs: Vec::new(),
        }))
    }
}

#[derive(Debug)]
struct ForEachAcc {
    nested: AggRef,
    function: String,
    accs: Vec<Box<dyn Accumulator>>,
}

impl ForEachAcc {
    fn grow(&mut self, n: usize) -> Result<()> {
        while self.accs.len() < n {
            self.accs.push(self.nested.accumulator()?);
        }
        Ok(())
    }
}

impl Accumulator for ForEachAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        let lens = lengths(values, &self.function)?;
        let max = lens.iter().copied().max().unwrap_or(0);
        self.grow(max)?;
        let starts: Vec<usize> = values[0].as_list::<i32>().offsets()[..lens.len()]
            .iter()
            .map(|o| *o as usize)
            .collect();
        let children: Vec<ArrayRef> = values
            .iter()
            .map(|a| a.as_list::<i32>().values().clone())
            .collect();
        for j in 0..max {
            let idx: UInt32Array = lens
                .iter()
                .zip(&starts)
                .filter(|(l, _)| **l > j)
                .map(|(_, s)| (s + j) as u32)
                .collect();
            let taken = children
                .iter()
                .map(|c| Ok(take(c.as_ref(), &idx, None)?))
                .collect::<Result<Vec<_>>>()?;
            let taken = decode(&taken)?;
            self.accs[j].update_batch(&taken)?;
        }
        Ok(())
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        let item = item_field(self.nested.result())?;
        let values = if self.accs.is_empty() {
            new_empty_array(item.data_type())
        } else {
            let scalars = self
                .accs
                .iter_mut()
                .map(|a| a.evaluate())
                .collect::<Result<Vec<_>>>()?;
            ScalarValue::iter_to_array(scalars)?
        };
        list_scalar(values, &item)
    }

    fn size(&self) -> usize {
        size_of::<Self>() + self.accs.iter().map(|a| a.size()).sum::<usize>()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        let rows = self
            .accs
            .iter_mut()
            .map(|a| a.state())
            .collect::<Result<Vec<_>>>()?;
        Ok(vec![state_struct_list(&self.nested.state(), rows)?])
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        for row in 0..states[0].len() {
            let cols = state_struct_rows(&states[0], row)?;
            let n = cols.first().map_or(0, |c| c.len());
            self.grow(n)?;
            for j in 0..n {
                let slice: Vec<ArrayRef> = cols.iter().map(|c| c.slice(j, 1)).collect();
                self.accs[j].merge_batch(&slice)?;
            }
        }
        Ok(())
    }
}

/// `-Distinct` (`AggregateFunctionDistinct.h`), and `DISTINCT` in a call:
/// each distinct tuple of arguments is aggregated once.
pub fn distinct(nested: AggRef, args: &[ChType]) -> AggRef {
    let args = args
        .iter()
        .map(|t| match t {
            ChType::LowCardinality(inner) => (**inner).clone(),
            other => other.clone(),
        })
        .collect();
    Arc::new(DistinctWrap { nested, args })
}

#[derive(Debug)]
struct DistinctWrap {
    nested: AggRef,
    args: Vec<ChType>,
}

impl DistinctWrap {
    fn types(&self) -> Vec<DataType> {
        self.args
            .iter()
            .map(|t| arrow_type(t).unwrap_or(DataType::Null))
            .collect()
    }
}

impl Agg for DistinctWrap {
    fn result(&self) -> &ChType {
        self.nested.result()
    }

    fn state(&self) -> Vec<DataType> {
        vec![state_struct_list_type(&self.types())]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        let types = self.types();
        Ok(Box::new(DistinctAcc {
            rows: RowConverter::new(types.iter().cloned().map(SortField::new).collect())?,
            types,
            nested: self.nested.clone(),
            seen: HashSet::new(),
            kept: Vec::new(),
        }))
    }

    fn default_when_only_null(&self) -> bool {
        self.nested.default_when_only_null()
    }
}

#[derive(Debug)]
struct DistinctAcc {
    rows: RowConverter,
    types: Vec<DataType>,
    nested: AggRef,
    seen: HashSet<OwnedRow>,
    kept: Vec<Vec<ArrayRef>>,
}

impl DistinctAcc {
    fn offer(&mut self, values: &[ArrayRef]) -> Result<()> {
        let values = values
            .iter()
            .zip(&self.types)
            .map(|(a, dt)| conform(a.clone(), dt))
            .collect::<Result<Vec<_>>>()?;
        let rows = self.rows.convert_columns(&values)?;
        let mut idx = Vec::new();
        for i in 0..rows.num_rows() {
            if self.seen.insert(rows.row(i).owned()) {
                idx.push(i as u32);
            }
        }
        if !idx.is_empty() {
            let idx = UInt32Array::from(idx);
            self.kept.push(
                values
                    .iter()
                    .map(|a| Ok(take(a.as_ref(), &idx, None)?))
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        Ok(())
    }

    fn columns(&self) -> Result<Vec<ArrayRef>> {
        (0..self.types.len())
            .map(|i| {
                if self.kept.is_empty() {
                    return Ok(new_empty_array(&self.types[i]));
                }
                let parts: Vec<&dyn Array> = self.kept.iter().map(|k| k[i].as_ref()).collect();
                Ok(concat(&parts)?)
            })
            .collect()
    }
}

impl Accumulator for DistinctAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        self.offer(values)
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        let mut acc = self.nested.accumulator()?;
        let columns = self.columns()?;
        if columns.first().is_some_and(|c| !c.is_empty()) {
            acc.update_batch(&columns)?;
        }
        acc.evaluate()
    }

    fn size(&self) -> usize {
        size_of::<Self>()
            + self
                .kept
                .iter()
                .flatten()
                .map(|a| a.get_array_memory_size())
                .sum::<usize>()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        let columns = self.columns()?;
        let fields = super::state_struct_fields(&self.types);
        let len = columns.first().map_or(0, |c| c.len());
        let array = if fields.is_empty() {
            StructArray::new_empty_fields(len, None)
        } else {
            StructArray::try_new(fields, columns, None)?
        };
        Ok(vec![super::state_list(Arc::new(array))?])
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        for row in 0..states[0].len() {
            let cols = state_struct_rows(&states[0], row)?;
            if !cols.is_empty() {
                self.offer(&cols)?;
            }
        }
        Ok(())
    }
}

/// `-OrNull` and `-OrDefault` (`AggregateFunctionOrFill.h`): with no input
/// row, `NULL` (when the result can be `Nullable`) or the result type's
/// default.
pub fn or_fill(nested: AggRef, null: bool) -> Result<AggRef, ChError> {
    let result = if null && can_be_inside_nullable(nested.result()) {
        ChType::Nullable(Box::new(nested.result().clone()))
    } else {
        nested.result().clone()
    };
    Ok(Arc::new(OrFill {
        null,
        result,
        nested,
    }))
}

#[derive(Debug)]
struct OrFill {
    null: bool,
    result: ChType,
    nested: AggRef,
}

impl Agg for OrFill {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        let mut s = vec![DataType::Boolean];
        s.extend(self.nested.state());
        s
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        Ok(Box::new(OrFillAcc {
            null: self.null,
            result: self.result.clone(),
            seen: false,
            nested: self.nested.accumulator()?,
        }))
    }

    fn default_when_only_null(&self) -> bool {
        !self.null && self.nested.default_when_only_null()
    }
}

#[derive(Debug)]
struct OrFillAcc {
    null: bool,
    result: ChType,
    seen: bool,
    nested: Box<dyn Accumulator>,
}

impl Accumulator for OrFillAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        if values.first().is_some_and(|a| !a.is_empty()) {
            self.seen = true;
        }
        self.nested.update_batch(values)
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        if self.seen {
            return self.nested.evaluate();
        }
        if self.null && self.result.is_nullable() {
            return null_scalar(&self.result);
        }
        default_scalar(&self.result)
    }

    fn size(&self) -> usize {
        size_of::<Self>() + self.nested.size()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        let mut s = vec![ScalarValue::Boolean(Some(self.seen))];
        s.extend(self.nested.state()?);
        Ok(s)
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        if states[0].as_boolean().iter().any(|b| b == Some(true)) {
            self.seen = true;
        }
        self.nested.merge_batch(&states[1..])
    }
}

/// The functions `-SimpleState` takes (26.9's error lists them).
const SIMPLE: &[&str] = &[
    "any",
    "anyLast",
    "min",
    "max",
    "sum",
    "sumWithOverflow",
    "groupBitAnd",
    "groupBitOr",
    "groupBitXor",
    "sumMap",
    "minMap",
    "maxMap",
    "groupArrayArray",
    "groupArrayLastArray",
    "groupUniqArrayArray",
    "sumMappedArrays",
    "minMappedArrays",
    "maxMappedArrays",
];

/// `-SimpleState`: the nested function's result as a
/// `SimpleAggregateFunction(f, T)` value.
pub fn simple_state(inner: &AggregateName, args: &[ChType]) -> Result<AggRef, ChError> {
    let function = inner.function();
    if !SIMPLE.contains(&function.as_str()) {
        return Err(ChError::bad_arguments(format!(
            "Unsupported aggregate function {function}, supported functions are {}",
            SIMPLE.join(",")
        )));
    }
    let nested = build(inner, args)?;
    let result = ChType::SimpleAggregateFunction(AggregateSpec {
        version: None,
        function,
        params: inner.params.iter().map(ToString::to_string).collect(),
        args: vec![nested.result().clone()],
    });
    Ok(Arc::new(Relabel { result, nested }))
}

/// The nested aggregate under another result type of the same layout.
#[derive(Debug)]
struct Relabel {
    result: ChType,
    nested: AggRef,
}

impl Agg for Relabel {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        self.nested.state()
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        self.nested.accumulator()
    }

    fn default_when_only_null(&self) -> bool {
        self.nested.default_when_only_null()
    }
}

/// `-Map` (`AggregateFunctionMap.cpp`): over a `Map(K, V)` argument, one
/// aggregate per key, keys in order; `sumMap`, `minMap` and `maxMap` over
/// arrays (or a tuple of arrays) are ClickHouse's mapped-arrays functions.
pub fn map(inner: &AggregateName, args: &[ChType], function: &str) -> Result<AggRef, ChError> {
    if let [ChType::Map(k, v)] = args {
        let nested = build(inner, &[(**v).clone()])?;
        let result = ChType::Map(k.clone(), Box::new(nested.result().clone()));
        return Ok(Arc::new(MapWrap {
            key: (**k).clone(),
            result,
            nested,
        }));
    }
    if inner.combinators.is_empty() && matches!(inner.base.as_str(), "sum" | "min" | "max") {
        if !inner.params.is_empty() {
            return Err(super::registry::no_params(function));
        }
        return super::maps::mapped_arrays(&inner.base, args);
    }
    Err(match args.first() {
        Some(t) => illegal_type(t, function),
        None => super::registry::arity(function, "one argument", 0),
    })
}

#[derive(Debug)]
struct MapWrap {
    key: ChType,
    result: ChType,
    nested: AggRef,
}

impl Agg for MapWrap {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        let mut types = vec![arrow_type(&self.key).unwrap_or(DataType::Null)];
        types.extend(self.nested.state());
        vec![state_struct_list_type(&types)]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        let kt = arrow_type(&self.key)?;
        let key_value = match &kt {
            DataType::Dictionary(_, v) => (**v).clone(),
            other => other.clone(),
        };
        Ok(Box::new(MapAcc {
            rows: RowConverter::new(vec![SortField::new(key_value.clone())])?,
            kt,
            key_value,
            result: self.result.clone(),
            nested: self.nested.clone(),
            accs: BTreeMap::new(),
        }))
    }
}

#[derive(Debug)]
struct MapAcc {
    rows: RowConverter,
    kt: DataType,
    key_value: DataType,
    result: ChType,
    nested: AggRef,
    accs: BTreeMap<OwnedRow, (ScalarValue, Box<dyn Accumulator>)>,
}

impl MapAcc {
    fn slot(
        &mut self,
        row: OwnedRow,
        key: &ArrayRef,
        i: usize,
    ) -> Result<&mut Box<dyn Accumulator>> {
        if !self.accs.contains_key(&row) {
            let k = ScalarValue::try_from_array(key, i)?;
            self.accs
                .insert(row.clone(), (k, self.nested.accumulator()?));
        }
        Ok(&mut self.accs.get_mut(&row).expect("inserted").1)
    }
}

impl Accumulator for MapAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        let m = values[0].as_map();
        let offsets = m.offsets();
        let start = offsets[0] as usize;
        let end = offsets[offsets.len() - 1] as usize;
        let keys = decode(&[m.keys().slice(start, end - start)])?.remove(0);
        let keys = conform(keys, &self.key_value)?;
        let vals = decode(&[m.values().slice(start, end - start)])?.remove(0);
        let rows = self.rows.convert_columns(std::slice::from_ref(&keys))?;
        let mut groups: BTreeMap<OwnedRow, (usize, Vec<u32>)> = BTreeMap::new();
        for i in 0..rows.num_rows() {
            groups
                .entry(rows.row(i).owned())
                .or_insert_with(|| (i, Vec::new()))
                .1
                .push(i as u32);
        }
        for (row, (first, idx)) in groups {
            let taken = take(vals.as_ref(), &UInt32Array::from(idx), None)?;
            self.slot(row, &keys, first)?.update_batch(&[taken])?;
        }
        Ok(())
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        let dt = arrow_type(&self.result)?;
        let DataType::Map(entries, sorted) = &dt else {
            return Err(ch(ChError::logical_error("a Map result of another layout")));
        };
        let DataType::Struct(fields) = entries.data_type() else {
            return Err(ch(ChError::logical_error(
                "Map entries that are not a struct",
            )));
        };
        let (keys, values) = if self.accs.is_empty() {
            (
                new_empty_array(fields[0].data_type()),
                new_empty_array(fields[1].data_type()),
            )
        } else {
            let keys = ScalarValue::iter_to_array(self.accs.values().map(|(k, _)| k.clone()))?;
            let vals = self
                .accs
                .values_mut()
                .map(|(_, a)| a.evaluate())
                .collect::<Result<Vec<_>>>()?;
            (
                conform(keys, fields[0].data_type())?,
                conform(ScalarValue::iter_to_array(vals)?, fields[1].data_type())?,
            )
        };
        let n = keys.len();
        let entries_array = StructArray::try_new(fields.clone(), vec![keys, values], None)?;
        let map = MapArray::try_new(
            entries.clone(),
            OffsetBuffer::from_lengths([n]),
            entries_array,
            None,
            *sorted,
        )?;
        Ok(ScalarValue::Map(Arc::new(map)))
    }

    fn size(&self) -> usize {
        size_of::<Self>() + self.accs.values().map(|(_, a)| a.size()).sum::<usize>()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        let mut types = vec![self.kt.clone()];
        types.extend(self.nested.state());
        let mut rows = Vec::with_capacity(self.accs.len());
        for (k, a) in self.accs.values_mut() {
            let mut r = vec![k.clone()];
            r.extend(a.state()?);
            rows.push(r);
        }
        Ok(vec![state_struct_list(&types, rows)?])
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        for row in 0..states[0].len() {
            let cols = state_struct_rows(&states[0], row)?;
            let Some((keys, rest)) = cols.split_first() else {
                continue;
            };
            let keys = decode(std::slice::from_ref(keys))?.remove(0);
            let keys = conform(keys, &self.key_value)?;
            let rows = self.rows.convert_columns(std::slice::from_ref(&keys))?;
            for j in 0..keys.len() {
                let slice: Vec<ArrayRef> = rest.iter().map(|c| c.slice(j, 1)).collect();
                self.slot(rows.row(j).owned(), &keys, j)?
                    .merge_batch(&slice)?;
            }
        }
        Ok(())
    }
}
