//! `groupArray[(max_size)]` and `groupUniqArray[(max_size)]`
//! (`AggregateFunctionGroupArray.cpp`, `AggregateFunctionGroupUniqArray.cpp`
//! in ClickHouse's `src/AggregateFunctions`, Apache-2.0).
//!
//! `groupArray` keeps the values in the order it reads them, up to
//! `max_size`. `groupUniqArray` keeps each distinct value once, in the order
//! first read; ClickHouse returns them in its hash table's order, which it
//! does not specify (R28.3).

use std::collections::HashSet;
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, UInt32Array, new_empty_array};
use datafusion::arrow::compute::{concat, take};
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::row::{OwnedRow, RowConverter, SortField};
use datafusion::common::{Result, ScalarValue};
use datafusion::logical_expr::Accumulator;
use loams_house_sql::aggregates::Param;
use loams_house_types::{ChError, ChType};

use super::registry::arity;
use super::{
    Agg, AggRef, arrow_type, ch, conform, item_field, list_scalar, state_list, state_list_type,
};

/// ClickHouse's `AGGREGATE_FUNCTION_GROUP_ARRAY_MAX_ELEMENT_SIZE`.
const MAX_SIZE: u64 = 0xFF_FFFF;

fn max_size(function: &str, params: &[Param]) -> Result<Option<u64>, ChError> {
    match params {
        [] => Ok(None),
        [p] => match p.as_u64() {
            Some(n) if n > 0 && n <= MAX_SIZE => Ok(Some(n)),
            Some(n) if n > MAX_SIZE => Err(ChError {
                code: 128,
                name: "TOO_LARGE_ARRAY_SIZE",
                message: format!("Too large array size (maximum: {MAX_SIZE})"),
            }),
            _ => Err(ChError::bad_arguments(format!(
                "Parameter for aggregate function {function} should be positive number"
            ))),
        },
        _ => Err(ChError::number_of_arguments_doesnt_match(format!(
            "Incorrect number of parameters for aggregate function {function}, should be 0 or 1"
        ))),
    }
}

/// `groupArray`.
pub fn group_array(params: &[Param], args: &[ChType]) -> Result<AggRef, ChError> {
    collect("groupArray", false, params, args)
}

/// `groupUniqArray`.
pub fn group_uniq_array(params: &[Param], args: &[ChType]) -> Result<AggRef, ChError> {
    collect("groupUniqArray", true, params, args)
}

fn collect(
    function: &str,
    unique: bool,
    params: &[Param],
    args: &[ChType],
) -> Result<AggRef, ChError> {
    let limit = max_size(function, params)?;
    let [t] = args else {
        return Err(arity(function, "one argument", args.len()));
    };
    Ok(Arc::new(Collect {
        unique,
        limit,
        element: t.clone(),
        result: ChType::Array(Box::new(t.clone())),
    }))
}

#[derive(Debug)]
struct Collect {
    unique: bool,
    limit: Option<u64>,
    element: ChType,
    result: ChType,
}

impl Agg for Collect {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        vec![state_list_type(
            &arrow_type(&self.element).unwrap_or(DataType::Null),
        )]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        let dt = arrow_type(&self.element)?;
        Ok(Box::new(CollectAcc {
            rows: if self.unique {
                Some(RowConverter::new(vec![SortField::new(dt.clone())])?)
            } else {
                None
            },
            seen: HashSet::new(),
            limit: self.limit.map_or(usize::MAX, |n| n as usize),
            element: self.element.clone(),
            dt,
            parts: Vec::new(),
            len: 0,
        }))
    }

    fn default_when_only_null(&self) -> bool {
        true
    }
}

#[derive(Debug)]
struct CollectAcc {
    rows: Option<RowConverter>,
    seen: HashSet<OwnedRow>,
    limit: usize,
    element: ChType,
    dt: DataType,
    parts: Vec<ArrayRef>,
    len: usize,
}

impl CollectAcc {
    fn add(&mut self, array: &ArrayRef) -> Result<()> {
        if self.len >= self.limit || array.is_empty() {
            return Ok(());
        }
        let array = conform(array.clone(), &self.dt)?;
        let room = self.limit - self.len;
        let part = match &self.rows {
            None => array.slice(0, array.len().min(room)),
            Some(conv) => {
                let rows = conv.convert_columns(std::slice::from_ref(&array))?;
                let mut idx = Vec::new();
                for i in 0..rows.num_rows() {
                    if idx.len() >= room {
                        break;
                    }
                    if self.seen.insert(rows.row(i).owned()) {
                        idx.push(i as u32);
                    }
                }
                take(array.as_ref(), &UInt32Array::from(idx), None)?
            }
        };
        self.len += part.len();
        if !part.is_empty() {
            self.parts.push(part);
        }
        Ok(())
    }

    fn values(&self) -> Result<ArrayRef> {
        if self.parts.is_empty() {
            return Ok(new_empty_array(&self.dt));
        }
        let parts: Vec<&dyn Array> = self.parts.iter().map(|p| p.as_ref()).collect();
        Ok(concat(&parts)?)
    }
}

impl Accumulator for CollectAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        self.add(&values[0])
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        let item = item_field(&self.element)?;
        list_scalar(self.values()?, &item)
    }

    fn size(&self) -> usize {
        size_of::<Self>()
            + self
                .parts
                .iter()
                .map(|p| p.get_array_memory_size())
                .sum::<usize>()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        Ok(vec![state_list(self.values()?)?])
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        for row in 0..states[0].len() {
            if let Some(values) = super::state_list_row(&states[0], row) {
                self.add(&values)?;
            }
        }
        Ok(())
    }
}
