//! The core aggregates: `count`, `sum`, `avg`, `min`, `max`, `any`,
//! `anyLast`, `argMin` and `argMax`, with ClickHouse's result types
//! (`AggregateFunctionSum.h`, `AggregateFunctionAvg.h`,
//! `AggregateFunctionCount.h`, `AggregateFunctionsSingleValue.cpp`,
//! `AggregateFunctionArgMinMax.cpp` in ClickHouse's `src/AggregateFunctions`,
//! Apache-2.0).

use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, Decimal128Array, Decimal256Array, Int64Array, UInt64Array,
};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{
    DataType, Decimal128Type, Decimal256Type, Float64Type, Int64Type, UInt64Type, i256,
};
use datafusion::arrow::row::{OwnedRow, RowConverter, SortField};
use datafusion::common::{Result, ScalarValue};
use datafusion::logical_expr::Accumulator;
use loams_house_types::{ChError, ChType};

use super::registry::illegal_type;
use super::{Agg, arrow_type, default_scalar};

/// `count`: the rows it is given. `NULL`s never reach it (the `Null`
/// combinator skips them), and on `NULL`-only input it is `0`.
#[derive(Debug)]
pub struct Count {
    result: ChType,
}

impl Count {
    /// `count` over `args` (any types).
    pub fn new(_args: &[ChType]) -> Self {
        Self {
            result: ChType::UInt64,
        }
    }
}

impl Agg for Count {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        vec![DataType::UInt64]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        Ok(Box::new(CountAcc(0)))
    }

    fn default_when_only_null(&self) -> bool {
        true
    }
}

#[derive(Debug)]
struct CountAcc(u64);

impl Accumulator for CountAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        if let Some(a) = values.first() {
            self.0 += a.len() as u64;
        }
        Ok(())
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        Ok(ScalarValue::UInt64(Some(self.0)))
    }

    fn size(&self) -> usize {
        size_of::<Self>()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        Ok(vec![ScalarValue::UInt64(Some(self.0))])
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        let a = states[0].as_primitive::<UInt64Type>();
        self.0 = a.iter().flatten().fold(self.0, u64::wrapping_add);
        Ok(())
    }
}

/// How a sum accumulates: ClickHouse's `AggregateFunctionSumData` over the
/// result's native type, wrapping on overflow.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SumKind {
    U64,
    I64,
    F64,
    Dec128(u8),
    Dec256(u8),
}

impl SumKind {
    /// The sum's kind and result type for an argument of `t` (`sum(UInt8)` is
    /// `UInt64`, `sum(Decimal(9, 2))` is `Decimal(38, 2)`).
    fn of(t: &ChType, function: &str) -> Result<(Self, ChType), ChError> {
        Ok(match t {
            ChType::UInt8 | ChType::UInt16 | ChType::UInt32 | ChType::UInt64 | ChType::Bool => {
                (Self::U64, ChType::UInt64)
            }
            ChType::Int8 | ChType::Int16 | ChType::Int32 | ChType::Int64 => {
                (Self::I64, ChType::Int64)
            }
            ChType::Float32 | ChType::Float64 => (Self::F64, ChType::Float64),
            ChType::Decimal { precision, scale } if *precision <= 38 => (
                Self::Dec128(*scale),
                ChType::Decimal {
                    precision: 38,
                    scale: *scale,
                },
            ),
            ChType::Decimal { scale, .. } => (
                Self::Dec256(*scale),
                ChType::Decimal {
                    precision: 76,
                    scale: *scale,
                },
            ),
            ChType::UInt128 | ChType::UInt256 | ChType::Int128 | ChType::Int256 => {
                return Err(ChError::unsupported(format!("{function} of {t}")));
            }
            other => return Err(illegal_type(other, function)),
        })
    }

    fn arrow(self) -> DataType {
        match self {
            Self::U64 => DataType::UInt64,
            Self::I64 => DataType::Int64,
            Self::F64 => DataType::Float64,
            Self::Dec128(s) => DataType::Decimal128(38, s as i8),
            Self::Dec256(s) => DataType::Decimal256(76, s as i8),
        }
    }
}

/// A running sum.
#[derive(Debug, Clone, Copy)]
enum Total {
    U64(u64),
    I64(i64),
    F64(f64),
    Dec128(i128),
    Dec256(i256),
}

impl Total {
    fn zero(kind: SumKind) -> Self {
        match kind {
            SumKind::U64 => Self::U64(0),
            SumKind::I64 => Self::I64(0),
            SumKind::F64 => Self::F64(0.0),
            SumKind::Dec128(_) => Self::Dec128(0),
            SumKind::Dec256(_) => Self::Dec256(i256::ZERO),
        }
    }

    /// Adds `array` (already of the kind's Arrow type), as ClickHouse's
    /// `addMany` does for one block: floats in sixteen partial sums, then the
    /// tail, so the rounding matches.
    fn add_array(&mut self, array: &ArrayRef) {
        match self {
            Self::U64(s) => {
                let a = array.as_primitive::<UInt64Type>();
                *s = a.iter().flatten().fold(*s, u64::wrapping_add);
            }
            Self::I64(s) => {
                let a = array.as_primitive::<Int64Type>();
                *s = a.iter().flatten().fold(*s, i64::wrapping_add);
            }
            Self::F64(s) => {
                let a = array.as_primitive::<Float64Type>();
                if a.null_count() == 0 {
                    float_sum(s, a.values());
                } else {
                    let v: Vec<f64> = a.iter().flatten().collect();
                    float_sum(s, &v);
                }
            }
            Self::Dec128(s) => {
                let a = array.as_primitive::<Decimal128Type>();
                *s = a.iter().flatten().fold(*s, i128::wrapping_add);
            }
            Self::Dec256(s) => {
                let a = array.as_primitive::<Decimal256Type>();
                *s = a.iter().flatten().fold(*s, i256::wrapping_add);
            }
        }
    }

    fn scalar(self, kind: SumKind) -> ScalarValue {
        match (self, kind) {
            (Self::U64(v), _) => ScalarValue::UInt64(Some(v)),
            (Self::I64(v), _) => ScalarValue::Int64(Some(v)),
            (Self::F64(v), _) => ScalarValue::Float64(Some(v)),
            (Self::Dec128(v), SumKind::Dec128(s)) => ScalarValue::Decimal128(Some(v), 38, s as i8),
            (Self::Dec256(v), SumKind::Dec256(s)) => ScalarValue::Decimal256(Some(v), 76, s as i8),
            (Self::Dec128(v), _) => ScalarValue::Decimal128(Some(v), 38, 0),
            (Self::Dec256(v), _) => ScalarValue::Decimal256(Some(v), 76, 0),
        }
    }
}

/// ClickHouse's `AggregateFunctionSumData::addManyImpl` for floats: sixteen
/// partial sums over the whole chunks of sixteen, added in order, then the
/// tail summed on its own and added.
pub fn float_sum(sum: &mut f64, values: &[f64]) {
    const LANES: usize = 16;
    let whole = values.len() / LANES * LANES;
    let mut partial = [0f64; LANES];
    for chunk in values[..whole].chunks_exact(LANES) {
        for (p, v) in partial.iter_mut().zip(chunk) {
            *p += v;
        }
    }
    for p in partial {
        *sum += p;
    }
    let mut local = 0f64;
    for v in &values[whole..] {
        local += v;
    }
    *sum += local;
}

/// `sum`.
#[derive(Debug)]
pub struct Sum {
    kind: SumKind,
    result: ChType,
}

impl Sum {
    /// `sum(x)`.
    pub fn new(args: &[ChType]) -> Result<Self, ChError> {
        let [t] = args else {
            return Err(super::registry::arity("sum", "one argument", args.len()));
        };
        let (kind, result) = SumKind::of(t, "sum")?;
        Ok(Self { kind, result })
    }
}

impl Agg for Sum {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        vec![self.kind.arrow()]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        Ok(Box::new(SumAcc {
            kind: self.kind,
            total: Total::zero(self.kind),
        }))
    }
}

#[derive(Debug)]
struct SumAcc {
    kind: SumKind,
    total: Total,
}

impl Accumulator for SumAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        let a = cast(&values[0], &self.kind.arrow())?;
        self.total.add_array(&a);
        Ok(())
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        Ok(self.total.scalar(self.kind))
    }

    fn size(&self) -> usize {
        size_of::<Self>()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        Ok(vec![self.total.scalar(self.kind)])
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        // Partial sums add one at a time, as ClickHouse's `merge` does.
        if let Total::F64(s) = &mut self.total {
            for v in states[0].as_primitive::<Float64Type>().iter().flatten() {
                *s += v;
            }
            return Ok(());
        }
        self.total.add_array(&states[0]);
        Ok(())
    }
}

/// How `avg` accumulates its numerator (`AvgFieldType`): integers and dates
/// exactly, floats as `sum` does, decimals in their 128- or 256-bit form.
#[derive(Debug, Clone, Copy, PartialEq)]
enum AvgKind {
    /// Integers; the numerator is kept in 128 bits.
    Int,
    Float,
    Decimal128(u8),
    Decimal256(u8),
    /// `Date`, `Date32`, `DateTime`: the result keeps the type, rounded half
    /// to even.
    Date,
    /// `DateTime64(p)`: ticks, rounded half to even.
    DateTime64(u8),
}

/// `avg`: `Float64`, or the argument's type for dates and times (26.9).
#[derive(Debug)]
pub struct Avg {
    kind: AvgKind,
    arg: ChType,
    result: ChType,
}

impl Avg {
    /// `avg(x)`.
    pub fn new(args: &[ChType]) -> Result<Self, ChError> {
        let [t] = args else {
            return Err(super::registry::arity("avg", "one argument", args.len()));
        };
        let (kind, result) = match t {
            ChType::UInt8
            | ChType::UInt16
            | ChType::UInt32
            | ChType::UInt64
            | ChType::Int8
            | ChType::Int16
            | ChType::Int32
            | ChType::Int64
            | ChType::Bool => (AvgKind::Int, ChType::Float64),
            ChType::Float32 | ChType::Float64 => (AvgKind::Float, ChType::Float64),
            ChType::Decimal { precision, scale } if *precision <= 38 => {
                (AvgKind::Decimal128(*scale), ChType::Float64)
            }
            ChType::Decimal { scale, .. } => (AvgKind::Decimal256(*scale), ChType::Float64),
            ChType::Date | ChType::Date32 | ChType::DateTime(_) => (AvgKind::Date, t.clone()),
            ChType::DateTime64(p, _) => (AvgKind::DateTime64(*p), t.clone()),
            other => return Err(illegal_type(other, "avg")),
        };
        Ok(Self {
            kind,
            arg: t.clone(),
            result,
        })
    }

    fn numerator_type(&self) -> DataType {
        match self.kind {
            AvgKind::Float => DataType::Float64,
            AvgKind::Decimal256(_) => DataType::Decimal256(76, 0),
            _ => DataType::Decimal128(38, 0),
        }
    }
}

impl Agg for Avg {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        vec![self.numerator_type(), DataType::UInt64]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        Ok(Box::new(AvgAcc {
            kind: self.kind,
            arg: arrow_type(&self.arg)?,
            result: self.result.clone(),
            int: 0,
            float: 0.0,
            wide: i256::ZERO,
            count: 0,
        }))
    }
}

#[derive(Debug)]
struct AvgAcc {
    kind: AvgKind,
    arg: DataType,
    result: ChType,
    int: i128,
    float: f64,
    wide: i256,
    count: u64,
}

/// `num / den` rounded half to even (ClickHouse's
/// `divideRoundHalfToEvenClamped`).
fn div_half_even(num: i128, den: i128) -> i128 {
    let q = num.div_euclid(den);
    let r = num.rem_euclid(den);
    match (2 * r).cmp(&den) {
        std::cmp::Ordering::Less => q,
        std::cmp::Ordering::Greater => q + 1,
        std::cmp::Ordering::Equal => {
            if q % 2 == 0 {
                q
            } else {
                q + 1
            }
        }
    }
}

impl AvgAcc {
    fn add_values(&mut self, a: &ArrayRef) -> Result<()> {
        match self.kind {
            AvgKind::Float => {
                let f = cast(a, &DataType::Float64)?;
                let f = f.as_primitive::<Float64Type>();
                let v: Vec<f64> = f.iter().flatten().collect();
                // `AggregateFunctionAvg::addBatchSinglePlace` sums each block
                // with `AggregateFunctionSumData` and adds the block's sum.
                let mut block = 0.0;
                float_sum(&mut block, &v);
                self.float += block;
                self.count += v.len() as u64;
            }
            AvgKind::Decimal256(s) => {
                let d = cast(a, &DataType::Decimal256(76, s as i8))?;
                for v in d.as_primitive::<Decimal256Type>().iter().flatten() {
                    self.wide = self.wide.wrapping_add(v);
                    self.count += 1;
                }
            }
            AvgKind::Decimal128(s) => {
                let d = cast(a, &DataType::Decimal128(38, s as i8))?;
                for v in d.as_primitive::<Decimal128Type>().iter().flatten() {
                    self.int = self.int.wrapping_add(v);
                    self.count += 1;
                }
            }
            AvgKind::Int | AvgKind::Date | AvgKind::DateTime64(_) => {
                if a.data_type() == &DataType::UInt64 {
                    for v in a.as_primitive::<UInt64Type>().iter().flatten() {
                        self.int = self.int.wrapping_add(i128::from(v));
                        self.count += 1;
                    }
                } else {
                    let ints = cast(a, &DataType::Int64)?;
                    for v in ints.as_primitive::<Int64Type>().iter().flatten() {
                        self.int = self.int.wrapping_add(i128::from(v));
                        self.count += 1;
                    }
                }
            }
        }
        Ok(())
    }
}

impl Accumulator for AvgAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        self.add_values(&values[0])
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        let den = self.count as f64;
        Ok(match self.kind {
            AvgKind::Float => ScalarValue::Float64(Some(self.float / den)),
            AvgKind::Int => ScalarValue::Float64(Some(self.int as f64 / den)),
            AvgKind::Decimal128(s) => {
                ScalarValue::Float64(Some(self.int as f64 / 10f64.powi(i32::from(s)) / den))
            }
            AvgKind::Decimal256(s) => {
                let num = self.wide.to_string().parse::<f64>().unwrap_or(f64::NAN);
                ScalarValue::Float64(Some(num / 10f64.powi(i32::from(s)) / den))
            }
            AvgKind::Date | AvgKind::DateTime64(_) => {
                if self.count == 0 {
                    return default_scalar(&self.result);
                }
                let v = div_half_even(self.int, i128::from(self.count));
                let array: ArrayRef = Arc::new(Int64Array::from(vec![v as i64]));
                let array = cast(&array, &self.arg)?;
                ScalarValue::try_from_array(&array, 0)?
            }
        })
    }

    fn size(&self) -> usize {
        size_of::<Self>()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        let num = match self.kind {
            AvgKind::Float => ScalarValue::Float64(Some(self.float)),
            AvgKind::Decimal256(_) => ScalarValue::Decimal256(Some(self.wide), 76, 0),
            _ => ScalarValue::Decimal128(Some(self.int), 38, 0),
        };
        Ok(vec![num, ScalarValue::UInt64(Some(self.count))])
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        match self.kind {
            AvgKind::Float => {
                for v in states[0].as_primitive::<Float64Type>().iter().flatten() {
                    self.float += v;
                }
            }
            AvgKind::Decimal256(_) => {
                let a: &Decimal256Array = states[0].as_primitive();
                for v in a.iter().flatten() {
                    self.wide = self.wide.wrapping_add(v);
                }
            }
            _ => {
                let a: &Decimal128Array = states[0].as_primitive();
                for v in a.iter().flatten() {
                    self.int = self.int.wrapping_add(v);
                }
            }
        }
        let c: &UInt64Array = states[1].as_primitive();
        self.count += c.iter().flatten().sum::<u64>();
        Ok(())
    }
}

/// `min`, `max`, `any`, `anyLast`: one of the values, of the argument's
/// type; the type's default on empty input. Values compare as ClickHouse
/// compares them (bytes for strings, numbers for enums), through Arrow's
/// row format.
#[derive(Debug)]
pub struct Pick {
    which: Which,
    result: ChType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Which {
    Min,
    Max,
    First,
    Last,
}

impl Pick {
    /// `function(x)` for `min`, `max`, `any` or `anyLast`.
    pub fn new(function: &str, args: &[ChType]) -> Result<Self, ChError> {
        let [t] = args else {
            return Err(super::registry::arity(function, "one argument", args.len()));
        };
        let which = match function {
            "min" => Which::Min,
            "max" => Which::Max,
            "any" => Which::First,
            _ => Which::Last,
        };
        Ok(Self {
            which,
            result: t.clone(),
        })
    }
}

impl Agg for Pick {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        vec![arrow_type(&self.result).unwrap_or(DataType::Null)]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        let dt = arrow_type(&self.result)?;
        Ok(Box::new(PickAcc {
            which: self.which,
            rows: RowConverter::new(vec![SortField::new(dt.clone())])?,
            dt,
            result: self.result.clone(),
            best: None,
        }))
    }
}

#[derive(Debug)]
struct PickAcc {
    which: Which,
    rows: RowConverter,
    dt: DataType,
    result: ChType,
    best: Option<(OwnedRow, ScalarValue)>,
}

impl PickAcc {
    fn offer(&mut self, array: &ArrayRef) -> Result<()> {
        let n = array.len();
        if n == 0 {
            return Ok(());
        }
        let candidate = match self.which {
            Which::First => {
                if self.best.is_some() {
                    return Ok(());
                }
                (0..n).find(|&i| array.is_valid(i))
            }
            Which::Last => (0..n).rev().find(|&i| array.is_valid(i)),
            Which::Min | Which::Max => {
                let rows = self.rows.convert_columns(std::slice::from_ref(array))?;
                let mut best: Option<usize> = None;
                for i in (0..n).filter(|&i| array.is_valid(i)) {
                    best = match best {
                        None => Some(i),
                        Some(b) => {
                            let better = if self.which == Which::Min {
                                rows.row(i) < rows.row(b)
                            } else {
                                rows.row(i) > rows.row(b)
                            };
                            Some(if better { i } else { b })
                        }
                    };
                }
                best
            }
        };
        let Some(i) = candidate else {
            return Ok(());
        };
        let slice = array.slice(i, 1);
        let row = self.rows.convert_columns(&[slice.clone()])?.row(0).owned();
        let replace = match (&self.best, self.which) {
            (None, _) => true,
            (Some(_), Which::First) => false,
            (Some(_), Which::Last) => true,
            (Some((b, _)), Which::Min) => row.row() < b.row(),
            (Some((b, _)), Which::Max) => row.row() > b.row(),
        };
        if replace {
            self.best = Some((row, ScalarValue::try_from_array(&slice, 0)?));
        }
        Ok(())
    }
}

impl Accumulator for PickAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        let a = super::conform(values[0].clone(), &self.dt)?;
        self.offer(&a)
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        match &self.best {
            Some((_, v)) => Ok(v.clone()),
            None => default_scalar(&self.result),
        }
    }

    fn size(&self) -> usize {
        size_of::<Self>() + self.best.as_ref().map_or(0, |(_, v)| v.size())
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        Ok(vec![match &self.best {
            Some((_, v)) => v.clone(),
            None => ScalarValue::try_from(&self.dt)?,
        }])
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        let a = super::conform(states[0].clone(), &self.dt)?;
        self.offer(&a)
    }
}

/// `argMin(v, k)` and `argMax(v, k)`: the `v` at the smallest (largest) `k`,
/// the first one on ties (`changeIfLess`/`changeIfGreater` are strict).
#[derive(Debug)]
pub struct ArgPick {
    max: bool,
    value: ChType,
    key: ChType,
}

impl ArgPick {
    /// `function(v, k)`.
    pub fn new(function: &str, args: &[ChType]) -> Result<Self, ChError> {
        let [v, k] = args else {
            return Err(super::registry::arity(
                function,
                "two arguments",
                args.len(),
            ));
        };
        Ok(Self {
            max: function == "argMax",
            value: v.clone(),
            key: k.clone(),
        })
    }
}

impl Agg for ArgPick {
    fn result(&self) -> &ChType {
        &self.value
    }

    fn state(&self) -> Vec<DataType> {
        vec![
            arrow_type(&self.value).unwrap_or(DataType::Null),
            arrow_type(&self.key).unwrap_or(DataType::Null),
        ]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        let kt = arrow_type(&self.key)?;
        Ok(Box::new(ArgPickAcc {
            max: self.max,
            rows: RowConverter::new(vec![SortField::new(kt.clone())])?,
            vt: arrow_type(&self.value)?,
            kt,
            result: self.value.clone(),
            best: None,
        }))
    }
}

#[derive(Debug)]
struct ArgPickAcc {
    max: bool,
    rows: RowConverter,
    vt: DataType,
    kt: DataType,
    result: ChType,
    best: Option<(OwnedRow, ScalarValue, ScalarValue)>,
}

impl ArgPickAcc {
    fn offer(&mut self, v: &ArrayRef, k: &ArrayRef) -> Result<()> {
        let k = super::conform(k.clone(), &self.kt)?;
        let v = super::conform(v.clone(), &self.vt)?;
        let rows = self.rows.convert_columns(std::slice::from_ref(&k))?;
        let mut best: Option<usize> = None;
        for i in (0..k.len()).filter(|&i| k.is_valid(i)) {
            let better = match best {
                None => true,
                Some(b) => {
                    if self.max {
                        rows.row(i) > rows.row(b)
                    } else {
                        rows.row(i) < rows.row(b)
                    }
                }
            };
            if better {
                best = Some(i);
            }
        }
        let Some(i) = best else {
            return Ok(());
        };
        let replace = match &self.best {
            None => true,
            Some((b, _, _)) => {
                if self.max {
                    rows.row(i) > b.row()
                } else {
                    rows.row(i) < b.row()
                }
            }
        };
        if replace {
            self.best = Some((
                rows.row(i).owned(),
                ScalarValue::try_from_array(&v, i)?,
                ScalarValue::try_from_array(&k, i)?,
            ));
        }
        Ok(())
    }
}

impl Accumulator for ArgPickAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        self.offer(&values[0], &values[1])
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        match &self.best {
            Some((_, v, _)) => Ok(v.clone()),
            None => default_scalar(&self.result),
        }
    }

    fn size(&self) -> usize {
        size_of::<Self>()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        Ok(match &self.best {
            Some((_, v, k)) => vec![v.clone(), k.clone()],
            None => vec![
                ScalarValue::try_from(&self.vt)?,
                ScalarValue::try_from(&self.kt)?,
            ],
        })
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        self.offer(&states[0], &states[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_even() {
        assert_eq!(div_half_even(5, 2), 2);
        assert_eq!(div_half_even(7, 2), 4);
        assert_eq!(div_half_even(-5, 2), -2);
        assert_eq!(div_half_even(10, 4), 2);
        assert_eq!(div_half_even(11, 4), 3);
    }

    #[test]
    fn float_sum_lanes() {
        let v: Vec<f64> = (0..40).map(|i| f64::from(i) * 0.1).collect();
        let mut s = 0.0;
        float_sum(&mut s, &v);
        let mut naive = 0.0;
        for x in &v {
            naive += x;
        }
        assert!((s - naive).abs() < 1e-9);
    }
}
