//! Seeding: a table exported from `clickhouse-server` as `Parquet` becomes a
//! table in the engine's Arrow layout (plan HE1 Task 23, D1804).
//!
//! ClickHouse's Parquet writer and the engine lay some types out differently
//! (`LowCardinality` as plain values, `Enum` as names or numbers, `DateTime`
//! as an integer or a timestamp in another unit, `Bool` as `UInt8`), so each
//! column is converted to [`loams_house_types::arrow::data_type`] of its
//! ClickHouse type, recursively through arrays, maps and tuples.

use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, Int8Array, Int16Array, ListArray, MapArray, RecordBatch, StructArray,
};
use datafusion::arrow::compute::{CastOptions, cast_with_options};
use datafusion::arrow::datatypes::{DataType, Int64Type, Schema};
use datafusion::parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use loams_house_types::arrow::{data_type, ticks_per_unit, to_arrow};
use loams_house_types::{ChError, ChType};

fn err(e: impl std::fmt::Display) -> ChError {
    ChError::cannot_parse_text(format!("Seeding: {e}"))
}

fn cast(array: &ArrayRef, to: &DataType) -> Result<ArrayRef, ChError> {
    cast_with_options(
        array,
        to,
        &CastOptions {
            safe: false,
            ..CastOptions::default()
        },
    )
    .map_err(|e| err(format!("{} to {to}: {e}", array.data_type())))
}

/// `array`, a column of `t` as ClickHouse's Parquet writer laid it out, in the
/// engine's layout.
///
/// # Errors
///
/// A layout this conversion does not know.
pub fn to_engine(t: &ChType, array: &ArrayRef) -> Result<ArrayRef, ChError> {
    let (target, _) = data_type(t)?;
    if array.data_type() == &target {
        return Ok(array.clone());
    }
    match t {
        ChType::Nullable(inner) => to_engine(inner, array),
        ChType::SimpleAggregateFunction(spec) => to_engine(&spec.args[0], array),
        ChType::LowCardinality(inner) => {
            let values = match array.data_type() {
                DataType::Dictionary(_, v) if v.as_ref() == &data_type(inner)?.0 => {
                    return cast(array, &target);
                }
                DataType::Dictionary(_, v) => cast(array, v)?,
                _ => array.clone(),
            };
            let plain = to_engine(inner, &values)?;
            cast(&plain, &target)
        }
        ChType::Enum8(values) => enum_values(
            array,
            |name| {
                values
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| i64::from(*v))
            },
            &target,
        ),
        ChType::Enum16(values) => enum_values(
            array,
            |name| {
                values
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| i64::from(*v))
            },
            &target,
        ),
        ChType::DateTime(_) | ChType::DateTime64(..) => {
            let precision = match t {
                ChType::DateTime64(p, _) => *p,
                _ => 0,
            };
            match array.data_type() {
                DataType::Timestamp(..) => cast(array, &target),
                d if d.is_integer() => {
                    // Ticks of 10^-precision seconds since the epoch.
                    let ticks = cast(array, &DataType::Int64)?;
                    let factor = ticks_per_unit(precision);
                    let scaled: datafusion::arrow::array::Int64Array =
                        ticks.as_primitive::<Int64Type>().unary(|v| v * factor);
                    cast(&(Arc::new(scaled) as ArrayRef), &target)
                }
                other => Err(err(format!("{other} for {t}"))),
            }
        }
        ChType::Array(inner) => {
            let list = array
                .as_list_opt::<i32>()
                .ok_or_else(|| err(format!("{} for {t}", array.data_type())))?;
            let DataType::List(field) = &target else {
                return Err(err("Array is not a List"));
            };
            let values = to_engine(inner, list.values())?;
            Ok(Arc::new(
                ListArray::try_new(
                    field.clone(),
                    list.offsets().clone(),
                    values,
                    list.nulls().cloned(),
                )
                .map_err(err)?,
            ))
        }
        ChType::Map(k, v) => {
            let map = array
                .as_map_opt()
                .ok_or_else(|| err(format!("{} for {t}", array.data_type())))?;
            let DataType::Map(field, sorted) = &target else {
                return Err(err("Map is not a Map"));
            };
            let DataType::Struct(fields) = field.data_type() else {
                return Err(err("Map entries are not a Struct"));
            };
            let keys = to_engine(k, map.keys())?;
            let vals = to_engine(v, map.values())?;
            let entries =
                StructArray::try_new(fields.clone(), vec![keys, vals], None).map_err(err)?;
            Ok(Arc::new(
                MapArray::try_new(
                    field.clone(),
                    map.offsets().clone(),
                    entries,
                    map.nulls().cloned(),
                    *sorted,
                )
                .map_err(err)?,
            ))
        }
        ChType::Tuple(elements) => {
            let st = array
                .as_struct_opt()
                .ok_or_else(|| err(format!("{} for {t}", array.data_type())))?;
            let DataType::Struct(fields) = &target else {
                return Err(err("Tuple is not a Struct"));
            };
            let children = elements
                .iter()
                .enumerate()
                .map(|(i, (_, et))| to_engine(et, st.column(i)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Arc::new(
                StructArray::try_new(fields.clone(), children, st.nulls().cloned()).map_err(err)?,
            ))
        }
        _ => cast(array, &target),
    }
}

fn enum_values(
    array: &ArrayRef,
    value_of: impl Fn(&str) -> Option<i64>,
    target: &DataType,
) -> Result<ArrayRef, ChError> {
    let strings = match array.data_type() {
        DataType::Utf8
        | DataType::LargeUtf8
        | DataType::Utf8View
        | DataType::Binary
        | DataType::LargeBinary
        | DataType::BinaryView => cast(array, &DataType::Utf8)?,
        // Numbers already: the enum's values.
        _ => return cast(array, target),
    };
    let strings = strings.as_string::<i32>();
    let mut out = Vec::with_capacity(strings.len());
    for i in 0..strings.len() {
        if strings.is_null(i) {
            out.push(None);
            continue;
        }
        let name = strings.value(i);
        out.push(Some(value_of(name).ok_or_else(|| {
            err(format!("{name:?} is not a value of the enum"))
        })?));
    }
    Ok(match target {
        DataType::Int8 => Arc::new(
            out.into_iter()
                .map(|v| v.map(|v| v as i8))
                .collect::<Int8Array>(),
        ),
        _ => Arc::new(
            out.into_iter()
                .map(|v| v.map(|v| v as i16))
                .collect::<Int16Array>(),
        ),
    })
}

/// Reads a `Parquet` export of a table whose columns are `columns` into
/// batches in the engine's layout, with the ClickHouse types in the schema.
///
/// # Errors
///
/// An unreadable file, a column count that does not match, or a column the
/// conversion does not know.
pub fn read_parquet(
    columns: &[(String, ChType)],
    parquet: bytes::Bytes,
) -> Result<(Arc<Schema>, Vec<RecordBatch>), ChError> {
    let fields = columns
        .iter()
        .map(|(n, t)| to_arrow(n, t))
        .collect::<Result<Vec<_>, _>>()?;
    let schema = Arc::new(Schema::new(fields));
    let reader = ParquetRecordBatchReaderBuilder::try_new(parquet)
        .map_err(err)?
        .build()
        .map_err(err)?;
    let mut batches = Vec::new();
    for batch in reader {
        let batch = batch.map_err(err)?;
        if batch.num_columns() != columns.len() {
            return Err(err(format!(
                "{} Parquet columns for {} table columns",
                batch.num_columns(),
                columns.len()
            )));
        }
        let arrays = columns
            .iter()
            .enumerate()
            .map(|(i, (name, t))| {
                to_engine(t, batch.column(i))
                    .map_err(|e| err(format!("column {name}: {}", e.message)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        batches.push(RecordBatch::try_new(schema.clone(), arrays).map_err(err)?);
    }
    Ok((schema, batches))
}
