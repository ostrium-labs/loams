//! ClickHouse's aggregate functions and combinators on DataFusion (plan HE1
//! Task 28).
//!
//! One DataFusion aggregate, [`ChAggregate`], stands for every ClickHouse
//! aggregate call. Its name is the call's planned name
//! (`loams_house_sql::aggregates::planned_name`: `sum`, `sumArrayIf`,
//! `topK(10)`), and [`registry::build`] turns that name and the arguments'
//! ClickHouse types into an [`Agg`]: the result type, the state's layout and
//! accumulators. The building follows ClickHouse's `AggregateFunctionFactory`:
//! `LowCardinality` arguments are read as their values; the combinators are
//! applied outermost first, each transforming the argument types for the one
//! inside; and wherever an argument is `Nullable`, the `Null` combinator skips
//! the rows holding `NULL` and makes the result `Nullable` unless the function
//! returns its default on such input (`count`) or its result cannot be
//! `Nullable` (`groupArray`).
//!
//! The outermost `-If` is planned as DataFusion's `FILTER (WHERE …)` by the
//! front end; every other combinator is a wrapper here ([`combinators`]), kept
//! generic over the [`Agg`] it wraps so Task 29 can add `-State` and `-Merge`.

pub mod arrays;
pub mod combinators;
pub mod core;
pub mod funnel;
pub mod maps;
pub mod registry;
pub mod topk;

use std::fmt::Debug;
use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, ListArray, StructArray, new_empty_array, new_null_array,
};
use datafusion::arrow::buffer::OffsetBuffer;
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{DataType, Field, FieldRef, Fields};
use datafusion::common::{DataFusionError, Result, ScalarValue};
use datafusion::logical_expr::function::{AccumulatorArgs, StateFieldsArgs};
use datafusion::logical_expr::utils::format_state_name;
use datafusion::logical_expr::{Accumulator, AggregateUDFImpl, Signature, Volatility};
use loams_house_sql::aggregates::{self, AggregateName};
use loams_house_types::{ChError, ChType};

/// A ClickHouse aggregate function, built for its argument types: what it
/// returns, how its state is laid out, and its accumulators.
pub trait Agg: Debug + Send + Sync {
    /// The ClickHouse result type.
    fn result(&self) -> &ChType;
    /// The Arrow types of the state's fields, in order.
    fn state(&self) -> Vec<DataType>;
    /// A fresh accumulator.
    fn accumulator(&self) -> Result<Box<dyn Accumulator>>;
    /// ClickHouse's `returns_default_when_only_null`: on `NULL`-only input it
    /// returns its default rather than `NULL` (`count`), so the `Null`
    /// combinator leaves its result type alone.
    fn default_when_only_null(&self) -> bool {
        false
    }
}

/// A built aggregate.
pub type AggRef = Arc<dyn Agg>;

/// A ClickHouse error carried through DataFusion.
pub fn ch(e: ChError) -> DataFusionError {
    DataFusionError::External(Box::new(e))
}

/// The Arrow type of a ClickHouse type in the engine's layout.
pub fn arrow_type(t: &ChType) -> Result<DataType> {
    Ok(loams_house_types::arrow::data_type(t).map_err(ch)?.0)
}

/// `array` with the Arrow type `dt`: unchanged when it has it, relabelled
/// when only field names, nullability flags or metadata differ, else cast.
pub fn conform(array: ArrayRef, dt: &DataType) -> Result<ArrayRef> {
    if array.data_type() == dt {
        return Ok(array);
    }
    if let Ok(data) = array.to_data().into_builder().data_type(dt.clone()).build() {
        return Ok(datafusion::arrow::array::make_array(data));
    }
    Ok(cast(&array, dt)?)
}

/// A one-row `List` scalar of `values` whose item field is `item`.
pub fn list_scalar(values: ArrayRef, item: &FieldRef) -> Result<ScalarValue> {
    let values = conform(values, item.data_type())?;
    let offsets = OffsetBuffer::from_lengths([values.len()]);
    let list = ListArray::try_new(item.clone(), offsets, values, None)?;
    Ok(ScalarValue::List(Arc::new(list)))
}

/// The item field of `Array(t)` in the engine's layout.
pub fn item_field(t: &ChType) -> Result<FieldRef> {
    Ok(Arc::new(
        loams_house_types::arrow::to_arrow("item", t).map_err(ch)?,
    ))
}

/// The item field of a state list: nullable, no metadata.
pub fn state_item(dt: &DataType) -> FieldRef {
    Arc::new(Field::new("item", dt.clone(), true))
}

/// The Arrow type of a state list of `dt`.
pub fn state_list_type(dt: &DataType) -> DataType {
    DataType::List(state_item(dt))
}

/// A state list scalar of `values`.
pub fn state_list(values: ArrayRef) -> Result<ScalarValue> {
    let item = state_item(values.data_type());
    list_scalar(values, &item)
}

/// The fields of a state struct holding `types` (named `0`, `1`, …).
pub fn state_struct_fields(types: &[DataType]) -> Fields {
    types
        .iter()
        .enumerate()
        .map(|(i, dt)| Field::new(i.to_string(), dt.clone(), true))
        .collect()
}

/// The Arrow type of a state list of structs of `types`.
pub fn state_struct_list_type(types: &[DataType]) -> DataType {
    state_list_type(&DataType::Struct(state_struct_fields(types)))
}

/// A state list of structs: one struct per row of `rows`, each row holding
/// one scalar per type of `types`.
pub fn state_struct_list(types: &[DataType], rows: Vec<Vec<ScalarValue>>) -> Result<ScalarValue> {
    let fields = state_struct_fields(types);
    let mut columns = Vec::with_capacity(types.len());
    for (i, dt) in types.iter().enumerate() {
        let column = if rows.is_empty() {
            new_empty_array(dt)
        } else {
            ScalarValue::iter_to_array(rows.iter().map(|r| r[i].clone()))?
        };
        columns.push(conform(column, dt)?);
    }
    let len = rows.len();
    let array = if fields.is_empty() {
        StructArray::new_empty_fields(len, None)
    } else {
        StructArray::try_new(fields, columns, None)?
    };
    state_list(Arc::new(array))
}

/// The structs in row `row` of a state list of structs, as columns.
pub fn state_struct_rows(list: &ArrayRef, row: usize) -> Result<Vec<ArrayRef>> {
    let list = list.as_list::<i32>();
    if list.is_null(row) {
        return Ok(Vec::new());
    }
    let value = list.value(row);
    Ok(value.as_struct().columns().to_vec())
}

/// The values in row `row` of a state list (empty when `NULL`).
pub fn state_list_row(list: &ArrayRef, row: usize) -> Option<ArrayRef> {
    let list = list.as_list::<i32>();
    (!list.is_null(row)).then(|| list.value(row))
}

/// A `NULL` of `t`'s layout.
pub fn null_scalar(t: &ChType) -> Result<ScalarValue> {
    let dt = arrow_type(t)?;
    ScalarValue::try_from_array(&new_null_array(&dt, 1), 0)
}

/// The default value of `t` (ClickHouse's `defaultValueOfTypeName`).
pub fn default_scalar(t: &ChType) -> Result<ScalarValue> {
    let array = loams_house_types::default::default_array(t).map_err(ch)?;
    let array = conform(array, &arrow_type(t)?)?;
    ScalarValue::try_from_array(&array, 0)
}

/// Whether ClickHouse lets `t` be inside `Nullable` (`canBeInsideNullable`).
pub fn can_be_inside_nullable(t: &ChType) -> bool {
    !matches!(
        t,
        ChType::Nullable(_)
            | ChType::LowCardinality(_)
            | ChType::Array(_)
            | ChType::Map(..)
            | ChType::Tuple(_)
            | ChType::Nested(_)
            | ChType::Json(_)
            | ChType::AggregateFunction(_)
            | ChType::Nothing
    )
}

/// The ClickHouse aggregate planned under one name.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct ChAggregate {
    name: String,
    signature: Signature,
}

impl ChAggregate {
    /// The aggregate planned as `name`, if `name` reads as an aggregate's
    /// planned name.
    pub fn new(name: &str) -> Option<Self> {
        aggregates::parse(name)?;
        Some(Self {
            name: name.to_string(),
            signature: Signature::variadic_any(Volatility::Immutable),
        })
    }

    fn spec(&self) -> Result<AggregateName> {
        aggregates::parse(&self.name).ok_or_else(|| {
            ch(ChError::unknown_function(format!(
                "Unknown aggregate function {}",
                self.name
            )))
        })
    }

    fn build(&self, fields: &[FieldRef], distinct: bool) -> Result<AggRef> {
        let types = fields
            .iter()
            .map(|f| loams_house_types::arrow::from_arrow(f).map_err(ch))
            .collect::<Result<Vec<_>>>()?;
        let spec = self.spec()?;
        let agg = registry::build(&spec, &types).map_err(ch)?;
        Ok(if distinct {
            combinators::distinct(agg, &types)
        } else {
            agg
        })
    }
}

impl AggregateUDFImpl for ChAggregate {
    fn name(&self) -> &str {
        &self.name
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, arg_types: &[DataType]) -> Result<DataType> {
        let fields: Vec<FieldRef> = arg_types
            .iter()
            .map(|dt| Arc::new(Field::new("", dt.clone(), false)))
            .collect();
        Ok(self.return_field(&fields)?.data_type().clone())
    }

    fn return_field(&self, arg_fields: &[FieldRef]) -> Result<FieldRef> {
        let agg = self.build(arg_fields, false)?;
        Ok(Arc::new(
            loams_house_types::arrow::to_arrow(&self.name, agg.result()).map_err(ch)?,
        ))
    }

    fn accumulator(&self, args: AccumulatorArgs) -> Result<Box<dyn Accumulator>> {
        let agg = self.build(args.expr_fields, args.is_distinct)?;
        Ok(Box::new(Decode {
            inner: agg.accumulator()?,
        }))
    }

    fn state_fields(&self, args: StateFieldsArgs) -> Result<Vec<FieldRef>> {
        let agg = self.build(args.input_fields, args.is_distinct)?;
        Ok(agg
            .state()
            .into_iter()
            .enumerate()
            .map(|(i, dt)| {
                Arc::new(Field::new(
                    format_state_name(args.name, &format!("ch{i}")),
                    dt,
                    true,
                ))
            })
            .collect())
    }
}

/// The outermost accumulator: reads `LowCardinality` arguments (Arrow
/// dictionaries) as their values, as ClickHouse's factory does.
#[derive(Debug)]
struct Decode {
    inner: Box<dyn Accumulator>,
}

pub(crate) fn decode(values: &[ArrayRef]) -> Result<Vec<ArrayRef>> {
    values
        .iter()
        .map(|a| match a.data_type() {
            DataType::Dictionary(_, v) => Ok(cast(a, v)?),
            _ => Ok(a.clone()),
        })
        .collect()
}

impl Accumulator for Decode {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        self.inner.update_batch(&decode(values)?)
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        self.inner.evaluate()
    }

    fn size(&self) -> usize {
        self.inner.size()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        self.inner.state()
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        self.inner.merge_batch(states)
    }
}

/// The engine's aggregate factory: [`ChAggregate`] for every planned
/// aggregate name.
#[derive(Debug, Default)]
pub struct Factory;

impl loams_house_sql::AggregateFactory for Factory {
    fn aggregate(&self, name: &str) -> Option<Arc<datafusion::logical_expr::AggregateUDF>> {
        ChAggregate::new(name).map(|a| Arc::new(datafusion::logical_expr::AggregateUDF::from(a)))
    }
}

/// Registers the aggregates with the engine's function registry.
pub fn register(r: &mut loams_house_sql::Registry) {
    r.aggregate_factory = Some(Arc::new(Factory));
}
