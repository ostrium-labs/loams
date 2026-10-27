//! `topK(N[, load_factor[, 'counts']])(x)` and `approx_top_k(N[,
//! reserved])(x)`: the Filtered Space-Saving algorithm, ported from
//! ClickHouse's `src/Common/SpaceSaving.h` and
//! `src/AggregateFunctions/AggregateFunctionTopK.cpp` (Apache-2.0).
//!
//! While at most `max(64, 2 × reserved)` distinct values are seen, the counts
//! are exact and the answer is ClickHouse's: the `N` values with the highest
//! `count - error`, then `count`. Values tied there come in the order first
//! seen; ClickHouse's `partial_sort` leaves their order unspecified. Past
//! that threshold the counters are truncated as in ClickHouse, with the
//! error spread through an `alpha` map indexed by a hash of the value that is
//! not ClickHouse's, so approximate counts can differ (R28.4).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, StructArray, UInt64Array, new_empty_array,
};
use datafusion::arrow::datatypes::{DataType, UInt64Type};
use datafusion::arrow::row::{OwnedRow, RowConverter, SortField};
use datafusion::common::{Result, ScalarValue};
use datafusion::logical_expr::Accumulator;
use loams_house_sql::aggregates::Param;
use loams_house_types::{ChError, ChType};

use super::registry::arity;
use super::{
    Agg, AggRef, arrow_type, ch, conform, item_field, list_scalar, state_list, state_list_type,
};

/// ClickHouse's `TOP_K_MAX_SIZE`.
const TOP_K_MAX_SIZE: u64 = 0xFF_FFFF;

fn out_of_bound(message: String) -> ChError {
    ChError::argument_out_of_bound(message)
}

/// `topK` or `approx_top_k` (`function`) with `params` over `args`.
pub fn top_k(function: &str, params: &[Param], args: &[ChType]) -> Result<AggRef, ChError> {
    let approx = function == "approx_top_k";
    let [t] = args else {
        return Err(arity(function, "one argument", args.len()));
    };
    let mut threshold = 10u64;
    let mut load_factor = 3u64;
    let mut include_counts = approx;
    let mut reserved = threshold * load_factor;
    if !params.is_empty() {
        if params.len() > 3 {
            return Err(ChError {
                code: 34,
                name: "TOO_MANY_ARGUMENTS_FOR_FUNCTION",
                message: format!(
                    "Aggregate function '{function}' requires three parameters or less"
                ),
            });
        }
        let number = |p: &Param| {
            p.as_u64().ok_or_else(|| {
                ChError::illegal_type_of_argument(format!(
                    "Parameter {p} of aggregate function '{function}' is not a number"
                ))
            })
        };
        threshold = number(&params[0])?;
        if params.len() >= 2 {
            if approx {
                reserved = number(&params[1])?;
                if reserved < 1 {
                    return Err(out_of_bound(format!(
                        "Too small parameter 'reserved' for aggregate function '{function}' (got {reserved}, minimum is 1)"
                    )));
                }
            } else {
                load_factor = number(&params[1])?;
                if load_factor < 1 {
                    return Err(out_of_bound(format!(
                        "Too small parameter 'load_factor' for aggregate function '{function}' (got {load_factor}, minimum is 1)"
                    )));
                }
            }
        }
        if params.len() == 3 {
            match params[2].as_str() {
                Some("counts") => include_counts = true,
                other => {
                    return Err(ChError::bad_arguments(format!(
                        "Aggregate function {function} doesn't support a parameter: {}",
                        other.unwrap_or("")
                    )));
                }
            }
        }
        if !approx || params.len() == 1 {
            reserved = threshold.saturating_mul(load_factor);
        }
        if reserved > TOP_K_MAX_SIZE || load_factor > TOP_K_MAX_SIZE || threshold > TOP_K_MAX_SIZE {
            return Err(out_of_bound(format!(
                "Too large parameter(s) for aggregate function '{function}' (maximum is {TOP_K_MAX_SIZE})"
            )));
        }
        if threshold == 0 || reserved == 0 {
            return Err(out_of_bound(format!(
                "Parameter 0 is illegal for aggregate function '{function}'"
            )));
        }
    }
    let element = if include_counts {
        ChType::Tuple(vec![
            (Some("item".into()), t.clone()),
            (Some("count".into()), ChType::UInt64),
            (Some("error".into()), ChType::UInt64),
        ])
    } else {
        t.clone()
    };
    Ok(Arc::new(TopK {
        threshold: threshold as usize,
        reserved: reserved as usize,
        include_counts,
        value: t.clone(),
        result: ChType::Array(Box::new(element.clone())),
        element,
    }))
}

#[derive(Debug)]
struct TopK {
    threshold: usize,
    reserved: usize,
    include_counts: bool,
    value: ChType,
    element: ChType,
    result: ChType,
}

impl Agg for TopK {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        vec![
            state_list_type(&arrow_type(&self.value).unwrap_or(DataType::Null)),
            state_list_type(&DataType::UInt64),
            state_list_type(&DataType::UInt64),
            state_list_type(&DataType::UInt64),
        ]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        let dt = arrow_type(&self.value)?;
        Ok(Box::new(TopKAcc {
            rows: RowConverter::new(vec![SortField::new(dt.clone())])?,
            dt,
            threshold: self.threshold,
            include_counts: self.include_counts,
            element: self.element.clone(),
            set: SpaceSaving::new(self.reserved),
        }))
    }
}

#[derive(Debug, Clone)]
struct Counter {
    key: OwnedRow,
    hash: u64,
    count: u64,
    error: u64,
}

impl Counter {
    /// `SpaceSaving::Counter::operator>`.
    fn greater(&self, b: &Counter) -> bool {
        let (x, y) = (
            self.count.wrapping_sub(self.error),
            b.count.wrapping_sub(b.error),
        );
        x > y || (x == y && self.count > b.count)
    }

    fn order(a: &Counter, b: &Counter) -> std::cmp::Ordering {
        if a.greater(b) {
            std::cmp::Ordering::Less
        } else if b.greater(a) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }
}

const MAX_ALPHA_VALUE: u64 = u32::MAX as u64;

#[derive(Debug)]
struct SpaceSaving {
    capacity: usize,
    target: usize,
    alpha: Vec<u64>,
    list: Vec<Counter>,
    map: HashMap<OwnedRow, usize>,
}

impl SpaceSaving {
    fn new(capacity: usize) -> Self {
        // `nextAlphaSize`: the power of two above `capacity × 6`.
        let x = (capacity as u64).saturating_mul(6);
        let alpha_size = 1usize << (64 - x.leading_zeros());
        Self {
            capacity,
            target: 64usize.max(capacity * 2),
            alpha: vec![0; alpha_size],
            list: Vec::new(),
            map: HashMap::new(),
        }
    }

    fn hash(key: &OwnedRow) -> u64 {
        let mut h = DefaultHasher::new();
        key.row().as_ref().hash(&mut h);
        h.finish()
    }

    fn insert(&mut self, key: OwnedRow, increment: u64) {
        if let Some(&i) = self.map.get(&key) {
            self.list[i].count += increment;
            return;
        }
        let hash = Self::hash(&key);
        if self.list.len() < self.capacity {
            self.push(Counter {
                key,
                hash,
                count: increment,
                error: 0,
            });
            return;
        }
        let alpha = self.alpha[(hash as usize) & (self.alpha.len() - 1)];
        self.push(Counter {
            key,
            hash,
            count: alpha + increment,
            error: alpha,
        });
    }

    fn push(&mut self, c: Counter) {
        self.map.insert(c.key.clone(), self.list.len());
        self.list.push(c);
        self.truncate(false);
    }

    fn truncate(&mut self, mut rebuild: bool) {
        if self.list.len() >= self.target {
            self.list.sort_by(Counter::order);
            let mask = self.alpha.len() - 1;
            for c in &self.list[self.capacity..] {
                let pos = (c.hash as usize) & mask;
                self.alpha[pos] = (self.alpha[pos] + c.count - c.error).min(MAX_ALPHA_VALUE);
            }
            self.list.truncate(self.capacity);
            rebuild = true;
        }
        if rebuild {
            self.map.clear();
            for (i, c) in self.list.iter().enumerate() {
                self.map.entry(c.key.clone()).or_insert(i);
            }
        }
    }

    /// `SpaceSaving::merge` (the Parallel Space Saving combine step).
    fn merge(&mut self, rhs: SpaceSaving) {
        if rhs.list.is_empty() {
            return;
        }
        if self.list.is_empty() {
            self.list = rhs.list;
            self.alpha = rhs.alpha;
            self.truncate(true);
            return;
        }
        let m1 = if self.list.len() >= self.capacity {
            self.alpha.iter().copied().max().unwrap_or(0)
        } else {
            0
        };
        let m2 = if rhs.list.len() >= rhs.capacity {
            rhs.alpha.iter().copied().max().unwrap_or(0)
        } else {
            0
        };
        if m2 > 0 {
            for c in &mut self.list {
                c.count += m2;
                c.error += m2;
            }
        }
        for c in rhs.list {
            if let Some(&i) = self.map.get(&c.key) {
                self.list[i].count += c.count - m2;
                self.list[i].error += c.error - m2;
            } else {
                self.list.push(Counter {
                    count: c.count + m1,
                    error: c.error + m1,
                    ..c
                });
            }
        }
        if self.alpha.len() == rhs.alpha.len() {
            for (a, b) in self.alpha.iter_mut().zip(&rhs.alpha) {
                *a = (*a + b).min(MAX_ALPHA_VALUE);
            }
        }
        self.truncate(true);
    }

    fn top(&self, k: usize) -> Vec<Counter> {
        let mut l = self.list.clone();
        l.sort_by(Counter::order);
        l.truncate(k);
        l
    }
}

#[derive(Debug)]
struct TopKAcc {
    rows: RowConverter,
    dt: DataType,
    threshold: usize,
    include_counts: bool,
    element: ChType,
    set: SpaceSaving,
}

impl TopKAcc {
    fn keys(&self, counters: &[Counter]) -> Result<ArrayRef> {
        if counters.is_empty() {
            return Ok(new_empty_array(&self.dt));
        }
        let mut out = self
            .rows
            .convert_rows(counters.iter().map(|c| c.key.row()))?;
        conform(out.remove(0), &self.dt)
    }
}

impl Accumulator for TopKAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        let a = conform(values[0].clone(), &self.dt)?;
        let rows = self.rows.convert_columns(&[a])?;
        for i in 0..rows.num_rows() {
            self.set.insert(rows.row(i).owned(), 1);
        }
        Ok(())
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        let top = self.set.top(self.threshold);
        let keys = self.keys(&top)?;
        let item = item_field(&self.element)?;
        if !self.include_counts {
            return list_scalar(keys, &item);
        }
        let DataType::Struct(fields) = item.data_type() else {
            return Err(ch(ChError::logical_error(
                "a tuple element of another layout",
            )));
        };
        let counts: ArrayRef = Arc::new(UInt64Array::from_iter_values(top.iter().map(|c| c.count)));
        let errors: ArrayRef = Arc::new(UInt64Array::from_iter_values(top.iter().map(|c| c.error)));
        let keys = conform(keys, fields[0].data_type())?;
        let s = StructArray::try_new(fields.clone(), vec![keys, counts, errors], None)?;
        list_scalar(Arc::new(s), &item)
    }

    fn size(&self) -> usize {
        size_of::<Self>() + self.set.list.len() * 64 + self.set.alpha.len() * 8
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        let l = &self.set.list;
        Ok(vec![
            state_list(self.keys(l)?)?,
            state_list(Arc::new(UInt64Array::from_iter_values(
                l.iter().map(|c| c.count),
            )))?,
            state_list(Arc::new(UInt64Array::from_iter_values(
                l.iter().map(|c| c.error),
            )))?,
            state_list(Arc::new(UInt64Array::from(self.set.alpha.clone())))?,
        ])
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        for row in 0..states[0].len() {
            let (Some(keys), Some(counts), Some(errors), Some(alpha)) = (
                super::state_list_row(&states[0], row),
                super::state_list_row(&states[1], row),
                super::state_list_row(&states[2], row),
                super::state_list_row(&states[3], row),
            ) else {
                continue;
            };
            let keys = conform(keys, &self.dt)?;
            let rows = self.rows.convert_columns(&[keys])?;
            let counts = counts.as_primitive::<UInt64Type>();
            let errors = errors.as_primitive::<UInt64Type>();
            let mut other = SpaceSaving::new(self.set.capacity);
            other.alpha = alpha.as_primitive::<UInt64Type>().values().to_vec();
            for i in 0..rows.num_rows() {
                let key = rows.row(i).owned();
                other.map.insert(key.clone(), other.list.len());
                other.list.push(Counter {
                    hash: SpaceSaving::hash(&key),
                    key,
                    count: counts.value(i),
                    error: errors.value(i),
                });
            }
            self.set.merge(other);
        }
        Ok(())
    }
}
