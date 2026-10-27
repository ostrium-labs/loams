//! `sumMap`, `minMap` and `maxMap` over arrays: ClickHouse's
//! `sumMappedArrays`, `minMappedArrays` and `maxMappedArrays`
//! (`AggregateFunctionSumMap.cpp`, Apache-2.0).
//!
//! `f(keys, values1, …)` (or `f((keys, values1, …))`) aggregates each values
//! array per key and answers `(sorted keys, values1', …)`. `sumMap` promotes
//! the values as `sum` does and leaves out a key whose sums are all zero;
//! `minMap` and `maxMap` keep every key and the values' type. The `Map(K, V)`
//! form is the `-Map` combinator ([`super::combinators::map`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, StructArray, new_empty_array};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::row::{OwnedRow, RowConverter, SortField};
use datafusion::common::{Result, ScalarValue};
use datafusion::logical_expr::Accumulator;
use loams_house_types::{ChError, ChType};

use super::registry::{arity, illegal_type};
use super::{
    Agg, AggRef, arrow_type, ch, conform, item_field, list_scalar, state_list, state_list_type,
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Sum,
    Min,
    Max,
}

/// `sumMap`/`minMap`/`maxMap` over arrays (`base` is `sum`, `min` or `max`).
pub fn mapped_arrays(base: &str, args: &[ChType]) -> Result<AggRef, ChError> {
    let function = format!("{base}Map");
    let op = match base {
        "sum" => Op::Sum,
        "min" => Op::Min,
        _ => Op::Max,
    };
    let (tuple, arrays): (bool, Vec<ChType>) = match args {
        [ChType::Tuple(elements)] => (true, elements.iter().map(|(_, t)| t.clone()).collect()),
        _ => (false, args.to_vec()),
    };
    if arrays.len() < 2 {
        return Err(arity(&function, "at least two arguments", arrays.len()));
    }
    let mut elems = Vec::with_capacity(arrays.len());
    for t in &arrays {
        match t {
            ChType::Array(inner) => elems.push((**inner).clone()),
            other => return Err(illegal_type(other, &function)),
        }
    }
    let key = elems[0].clone();
    let mut values = Vec::new();
    for v in &elems[1..] {
        let acc = match op {
            Op::Sum => match v.strip_nullable() {
                ChType::UInt8 | ChType::UInt16 | ChType::UInt32 | ChType::UInt64 | ChType::Bool => {
                    ChType::UInt64
                }
                ChType::Int8 | ChType::Int16 | ChType::Int32 | ChType::Int64 => ChType::Int64,
                ChType::Float32 | ChType::Float64 => ChType::Float64,
                ChType::Decimal { precision, scale } => ChType::Decimal {
                    precision: if *precision <= 38 { 38 } else { 76 },
                    scale: *scale,
                },
                other => return Err(illegal_type(other, &function)),
            },
            _ => v.strip_nullable().clone(),
        };
        values.push(acc);
    }
    let mut result = vec![(None, ChType::Array(Box::new(key.clone())))];
    result.extend(
        values
            .iter()
            .map(|v| (None, ChType::Array(Box::new(v.clone())))),
    );
    Ok(Arc::new(Mapped {
        op,
        tuple,
        key,
        values,
        result: ChType::Tuple(result),
        function,
    }))
}

#[derive(Debug)]
struct Mapped {
    op: Op,
    tuple: bool,
    key: ChType,
    values: Vec<ChType>,
    result: ChType,
    function: String,
}

impl Agg for Mapped {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        std::iter::once(&self.key)
            .chain(&self.values)
            .map(|t| state_list_type(&arrow_type(t).unwrap_or(DataType::Null)))
            .collect()
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        let kt = arrow_type(&self.key)?;
        Ok(Box::new(MappedAcc {
            op: self.op,
            tuple: self.tuple,
            rows: RowConverter::new(vec![SortField::new(kt.clone())])?,
            kt,
            vts: self
                .values
                .iter()
                .map(arrow_type)
                .collect::<Result<Vec<_>>>()?,
            key: self.key.clone(),
            values: self.values.clone(),
            result: self.result.clone(),
            function: self.function.clone(),
            entries: BTreeMap::new(),
        }))
    }
}

#[derive(Debug)]
struct MappedAcc {
    op: Op,
    tuple: bool,
    rows: RowConverter,
    kt: DataType,
    vts: Vec<DataType>,
    key: ChType,
    values: Vec<ChType>,
    result: ChType,
    function: String,
    entries: BTreeMap<OwnedRow, (ScalarValue, Vec<Option<ScalarValue>>)>,
}

impl MappedAcc {
    /// Adds flat `keys` and per-key `values` (already of the accumulators'
    /// types).
    fn add(&mut self, keys: &ArrayRef, values: &[ArrayRef]) -> Result<()> {
        let rows = self.rows.convert_columns(std::slice::from_ref(keys))?;
        for i in 0..keys.len() {
            let row = rows.row(i).owned();
            if !self.entries.contains_key(&row) {
                self.entries.insert(
                    row.clone(),
                    (
                        ScalarValue::try_from_array(keys, i)?,
                        vec![None; values.len()],
                    ),
                );
            }
            let slot = &mut self.entries.get_mut(&row).expect("inserted").1;
            for (c, v) in values.iter().enumerate() {
                if v.is_null(i) {
                    continue;
                }
                let x = ScalarValue::try_from_array(v, i)?;
                slot[c] = Some(match slot[c].take() {
                    None => x,
                    Some(cur) => match self.op {
                        Op::Sum => cur.add_checked(&x).or_else(|_| cur.add(&x))?,
                        Op::Min => {
                            if x.partial_cmp(&cur) == Some(std::cmp::Ordering::Less) {
                                x
                            } else {
                                cur
                            }
                        }
                        Op::Max => {
                            if x.partial_cmp(&cur) == Some(std::cmp::Ordering::Greater) {
                                x
                            } else {
                                cur
                            }
                        }
                    },
                });
            }
        }
        Ok(())
    }

    fn columns(&self, drop_zero: bool) -> Result<(ArrayRef, Vec<ArrayRef>)> {
        let kept: Vec<&(ScalarValue, Vec<Option<ScalarValue>>)> = self
            .entries
            .values()
            .filter(|(_, vals)| {
                !drop_zero
                    || vals
                        .iter()
                        .any(|v| v.as_ref().is_some_and(|v| !v.is_null() && !is_zero(v)))
            })
            .collect();
        if kept.is_empty() {
            return Ok((
                new_empty_array(&self.kt),
                self.vts.iter().map(new_empty_array).collect(),
            ));
        }
        let keys = conform(
            ScalarValue::iter_to_array(kept.iter().map(|(k, _)| k.clone()))?,
            &self.kt,
        )?;
        let mut cols = Vec::with_capacity(self.vts.len());
        for (c, dt) in self.vts.iter().enumerate() {
            let scalars = kept
                .iter()
                .map(|(_, v)| match &v[c] {
                    Some(s) => Ok(s.clone()),
                    None => ScalarValue::try_from(dt),
                })
                .collect::<Result<Vec<_>>>()?;
            cols.push(conform(ScalarValue::iter_to_array(scalars)?, dt)?);
        }
        Ok((keys, cols))
    }
}

fn is_zero(v: &ScalarValue) -> bool {
    match v {
        ScalarValue::UInt64(Some(x)) => *x == 0,
        ScalarValue::Int64(Some(x)) => *x == 0,
        ScalarValue::Float64(Some(x)) => *x == 0.0,
        ScalarValue::Decimal128(Some(x), ..) => *x == 0,
        ScalarValue::Decimal256(Some(x), ..) => *x == datafusion::arrow::datatypes::i256::ZERO,
        _ => false,
    }
}

impl Accumulator for MappedAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        let arrays: Vec<ArrayRef> = if self.tuple {
            values[0].as_struct().columns().to_vec()
        } else {
            values.to_vec()
        };
        let keys_list = arrays[0].as_list::<i32>();
        let n = keys_list.len();
        let mut flat_keys = Vec::new();
        let mut flat_vals: Vec<Vec<ArrayRef>> = vec![Vec::new(); arrays.len() - 1];
        for r in 0..n {
            if keys_list.is_null(r) {
                continue;
            }
            let k = keys_list.value(r);
            for (c, a) in arrays[1..].iter().enumerate() {
                let l = a.as_list::<i32>();
                let v = l.value(r);
                if v.len() != k.len() {
                    return Err(ch(ChError {
                        code: 190,
                        name: "SIZES_OF_ARRAYS_DONT_MATCH",
                        message: format!(
                            "Sizes of keys and values arrays do not match in aggregate function {}",
                            self.function
                        ),
                    }));
                }
                flat_vals[c].push(v);
            }
            flat_keys.push(k);
        }
        if flat_keys.is_empty() {
            return Ok(());
        }
        let refs: Vec<&dyn Array> = flat_keys.iter().map(|a| a.as_ref()).collect();
        let keys = super::decode(&[datafusion::arrow::compute::concat(&refs)?])?.remove(0);
        let keys = conform(keys, &self.kt)?;
        let mut vals = Vec::with_capacity(flat_vals.len());
        for (c, parts) in flat_vals.iter().enumerate() {
            let refs: Vec<&dyn Array> = parts.iter().map(|a| a.as_ref()).collect();
            let v = super::decode(&[datafusion::arrow::compute::concat(&refs)?])?.remove(0);
            vals.push(cast(&v, &self.vts[c])?);
        }
        self.add(&keys, &vals)
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        let (keys, cols) = self.columns(self.op == Op::Sum)?;
        let dt = arrow_type(&self.result)?;
        let DataType::Struct(fields) = dt else {
            return Err(ch(ChError::logical_error(
                "a tuple result of another layout",
            )));
        };
        let mut arrays = vec![list_scalar(keys, &item_field(&self.key)?)?.to_array()?];
        for (c, col) in cols.into_iter().enumerate() {
            arrays.push(list_scalar(col, &item_field(&self.values[c])?)?.to_array()?);
        }
        let s = StructArray::try_new(fields, arrays, None)?;
        Ok(ScalarValue::Struct(Arc::new(s)))
    }

    fn size(&self) -> usize {
        size_of::<Self>() + self.entries.len() * 64
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        let (keys, cols) = self.columns(false)?;
        let mut s = vec![state_list(keys)?];
        for c in cols {
            s.push(state_list(c)?);
        }
        Ok(s)
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        for row in 0..states[0].len() {
            let Some(keys) = super::state_list_row(&states[0], row) else {
                continue;
            };
            let vals = states[1..]
                .iter()
                .map(|s| {
                    super::state_list_row(s, row)
                        .unwrap_or_else(|| new_empty_array(&DataType::Null))
                })
                .collect::<Vec<_>>();
            self.add(&keys, &vals)?;
        }
        Ok(())
    }
}
