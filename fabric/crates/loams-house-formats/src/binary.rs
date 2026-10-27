//! ClickHouse's binary value encodings, shared by `RowBinary*` (one row at a
//! time) and `Native` (one column at a time), and the `RowBinary` writer.
//!
//! Each scalar is ClickHouse's `serializeBinary` (`src/DataTypes/Serializations/`,
//! Apache-2.0), checked against the native protocol's framing in
//! `loams-house-native`:
//!
//! * integers, floats, `Date` (`UInt16` days), `Date32` (`Int32` days),
//!   `DateTime` (`UInt32` seconds), `DateTime64(p)` (`Int64` ticks of 10^-p
//!   s), enums (`Int8`/`Int16`), `IPv4` (`UInt32`), intervals (`Int64`):
//!   little-endian;
//! * `Bool`: one byte; `Decimal(p, s)`: the unscaled integer in 4, 8, 16 or 32
//!   bytes by `p`;
//! * `String`: a `VarUInt` length and the bytes; `FixedString(N)`: N bytes;
//! * `UUID`: its two 64-bit halves, each little-endian (the text's bytes
//!   reversed per half); `IPv6`: 16 bytes in network order;
//! * `AggregateFunction`: the state's bytes as stored (D1800).
//!
//! `RowBinary` adds, per value: `Nullable`, a byte (1 for `NULL`, then
//! nothing; 0, then the value); `Array` and `Map`, a `VarUInt` length then the
//! elements (a map's as key, value pairs); `Tuple`, its elements in order;
//! `LowCardinality(T)`, `T`'s encoding.

use std::io::Write;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Decimal128Type, Decimal256Type, Float32Type, Float64Type, Int8Type, Int16Type,
    Int32Type, Int64Type, TimestampMicrosecondType, TimestampMillisecondType,
    TimestampNanosecondType, TimestampSecondType, UInt8Type, UInt16Type, UInt32Type, UInt64Type,
};
use arrow_array::{Array, RecordBatch};
use arrow_schema::{DataType, TimeUnit};
use loams_house_types::{ChError, ChType};

use crate::Column;
use crate::settings::FormatSettings;
use crate::writer::{FormatWriter, Stats, check_batch, io_error};

/// Appends a `VarUInt` (LEB128).
pub fn put_varuint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Appends a length-prefixed string.
pub fn put_string(out: &mut Vec<u8>, bytes: &[u8]) {
    put_varuint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

pub(crate) fn bad_array(t: &ChType, array: &dyn Array) -> ChError {
    ChError::logical_error(format!(
        "A column of {t} holds an Arrow {} array",
        array.data_type()
    ))
}

/// The bytes of a decimal of precision `p`.
pub(crate) fn decimal_width(precision: u8) -> usize {
    match precision {
        0..=9 => 4,
        10..=18 => 8,
        19..=38 => 16,
        _ => 32,
    }
}

/// When `array` is dictionary-encoded, the dictionary's values and the row's
/// key (`None`: a `NULL` key); otherwise `array` and `row`.
pub(crate) fn resolve(array: &dyn Array, row: usize) -> Option<(&dyn Array, usize)> {
    if let DataType::Dictionary(..) = array.data_type()
        && let Some(dict) = array.as_any_dictionary_opt()
    {
        if dict.keys().is_null(row) {
            return None;
        }
        let key = dict.normalized_keys()[row];
        return resolve(dict.values().as_ref(), key);
    }
    Some((array, row))
}

/// Whether row `row` of `array` is `NULL` (a dictionary's `NULL` key or
/// value included).
pub(crate) fn is_null(array: &dyn Array, row: usize) -> bool {
    match resolve(array, row) {
        None => true,
        Some((values, key)) => values.is_null(key),
    }
}

/// The bytes of a string-like value.
pub(crate) fn string_bytes<'a>(
    t: &ChType,
    array: &'a dyn Array,
    row: usize,
) -> Result<&'a [u8], ChError> {
    Ok(match array.data_type() {
        DataType::Utf8 => array.as_string::<i32>().value(row).as_bytes(),
        DataType::LargeUtf8 => array.as_string::<i64>().value(row).as_bytes(),
        DataType::Utf8View => array.as_string_view().value(row).as_bytes(),
        DataType::Binary => array.as_binary::<i32>().value(row),
        DataType::LargeBinary => array.as_binary::<i64>().value(row),
        DataType::BinaryView => array.as_binary_view().value(row),
        DataType::FixedSizeBinary(_) => array.as_fixed_size_binary().value(row),
        _ => return Err(bad_array(t, array)),
    })
}

fn unit_digits(unit: TimeUnit) -> u32 {
    match unit {
        TimeUnit::Second => 0,
        TimeUnit::Millisecond => 3,
        TimeUnit::Microsecond => 6,
        TimeUnit::Nanosecond => 9,
    }
}

/// A timestamp's value and unit.
fn timestamp(t: &ChType, array: &dyn Array, row: usize) -> Result<(i64, TimeUnit), ChError> {
    Ok(match array.data_type() {
        DataType::Timestamp(TimeUnit::Second, _) => (
            array.as_primitive::<TimestampSecondType>().value(row),
            TimeUnit::Second,
        ),
        DataType::Timestamp(TimeUnit::Millisecond, _) => (
            array.as_primitive::<TimestampMillisecondType>().value(row),
            TimeUnit::Millisecond,
        ),
        DataType::Timestamp(TimeUnit::Microsecond, _) => (
            array.as_primitive::<TimestampMicrosecondType>().value(row),
            TimeUnit::Microsecond,
        ),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => (
            array.as_primitive::<TimestampNanosecondType>().value(row),
            TimeUnit::Nanosecond,
        ),
        _ => return Err(bad_array(t, array)),
    })
}

/// A timestamp in ticks of 10^-`precision` seconds.
fn ticks(value: i64, unit: TimeUnit, precision: u8) -> i64 {
    let have = unit_digits(unit);
    let want = u32::from(precision);
    if want <= have {
        value.div_euclid(10i64.pow(have - want))
    } else {
        value.saturating_mul(10i64.pow(want - have))
    }
}

macro_rules! primitive {
    ($t:expr, $array:expr, $row:expr, $ty:ty) => {
        $array
            .as_primitive_opt::<$ty>()
            .ok_or_else(|| bad_array($t, $array))?
            .value($row)
    };
}

/// Appends the binary encoding of row `row` of `array`, a non-`NULL` value
/// of the scalar type `t` (no `Nullable`, `LowCardinality` or composite).
pub(crate) fn scalar(
    t: &ChType,
    array: &dyn Array,
    row: usize,
    out: &mut Vec<u8>,
) -> Result<(), ChError> {
    match t {
        ChType::UInt8 => out.push(primitive!(t, array, row, UInt8Type)),
        ChType::UInt16 => {
            out.extend_from_slice(&primitive!(t, array, row, UInt16Type).to_le_bytes())
        }
        ChType::UInt32 => {
            out.extend_from_slice(&primitive!(t, array, row, UInt32Type).to_le_bytes())
        }
        ChType::UInt64 => {
            out.extend_from_slice(&primitive!(t, array, row, UInt64Type).to_le_bytes())
        }
        ChType::Int8 => out.extend_from_slice(&primitive!(t, array, row, Int8Type).to_le_bytes()),
        ChType::Int16 => out.extend_from_slice(&primitive!(t, array, row, Int16Type).to_le_bytes()),
        ChType::Int32 => out.extend_from_slice(&primitive!(t, array, row, Int32Type).to_le_bytes()),
        ChType::Int64 | ChType::Interval(_) => {
            out.extend_from_slice(&primitive!(t, array, row, Int64Type).to_le_bytes())
        }
        ChType::Float32 => {
            out.extend_from_slice(&primitive!(t, array, row, Float32Type).to_le_bytes())
        }
        ChType::Float64 => {
            out.extend_from_slice(&primitive!(t, array, row, Float64Type).to_le_bytes())
        }
        ChType::Bool => {
            let v = array
                .as_boolean_opt()
                .ok_or_else(|| bad_array(t, array))?
                .value(row);
            out.push(u8::from(v));
        }
        ChType::Enum8(_) => {
            out.extend_from_slice(&primitive!(t, array, row, Int8Type).to_le_bytes())
        }
        ChType::Enum16(_) => {
            out.extend_from_slice(&primitive!(t, array, row, Int16Type).to_le_bytes())
        }
        ChType::IPv4 => out.extend_from_slice(&primitive!(t, array, row, UInt32Type).to_le_bytes()),
        ChType::Decimal { precision, .. } => {
            let width = decimal_width(*precision);
            match array.data_type() {
                DataType::Decimal128(..) => {
                    let v = array.as_primitive::<Decimal128Type>().value(row);
                    let bytes = v.to_le_bytes();
                    if width <= 16 {
                        out.extend_from_slice(&bytes[..width]);
                    } else {
                        // Sign-extend to 32 bytes.
                        out.extend_from_slice(&bytes);
                        let fill = if v < 0 { 0xff } else { 0 };
                        out.extend(std::iter::repeat_n(fill, width - 16));
                    }
                }
                DataType::Decimal256(..) => {
                    let bytes = array
                        .as_primitive::<Decimal256Type>()
                        .value(row)
                        .to_le_bytes();
                    out.extend_from_slice(&bytes[..width.min(32)]);
                }
                _ => return Err(bad_array(t, array)),
            }
        }
        ChType::String | ChType::Json(_) => put_string(out, string_bytes(t, array, row)?),
        ChType::FixedString(n) => {
            let bytes = string_bytes(t, array, row)?;
            let n = *n as usize;
            out.extend_from_slice(&bytes[..bytes.len().min(n)]);
            out.extend(std::iter::repeat_n(0, n.saturating_sub(bytes.len())));
        }
        ChType::AggregateFunction(_) => out.extend_from_slice(string_bytes(t, array, row)?),
        ChType::Uuid => {
            let b = string_bytes(t, array, row)?;
            if b.len() != 16 {
                return Err(bad_array(t, array));
            }
            out.extend(b[..8].iter().rev());
            out.extend(b[8..].iter().rev());
        }
        ChType::IPv6 => {
            let b = string_bytes(t, array, row)?;
            if b.len() != 16 {
                return Err(bad_array(t, array));
            }
            out.extend_from_slice(b);
        }
        ChType::Date => {
            let days = primitive!(t, array, row, Date32Type);
            out.extend_from_slice(&(days as u16).to_le_bytes());
        }
        ChType::Date32 => {
            out.extend_from_slice(&primitive!(t, array, row, Date32Type).to_le_bytes())
        }
        ChType::DateTime(_) => {
            let (v, unit) = timestamp(t, array, row)?;
            let secs = ticks(v, unit, 0);
            out.extend_from_slice(&(secs as u32).to_le_bytes());
        }
        ChType::DateTime64(p, _) => {
            let (v, unit) = timestamp(t, array, row)?;
            out.extend_from_slice(&ticks(v, unit, *p).to_le_bytes());
        }
        ChType::Nothing => out.push(b'0'),
        other => {
            return Err(ChError::unsupported(format!(
                "Writing values of {other} in a binary format"
            )));
        }
    }
    Ok(())
}

/// Appends the binary encoding of `t`'s default value (a `NULL`'s
/// placeholder in a `Nullable` column, and a `LowCardinality` dictionary's
/// first entries).
pub(crate) fn default_scalar(t: &ChType, out: &mut Vec<u8>) -> Result<(), ChError> {
    let width = match t {
        ChType::UInt8 | ChType::Int8 | ChType::Bool | ChType::Enum8(_) => 1,
        ChType::UInt16 | ChType::Int16 | ChType::Date | ChType::Enum16(_) => 2,
        ChType::UInt32
        | ChType::Int32
        | ChType::Float32
        | ChType::Date32
        | ChType::DateTime(_)
        | ChType::IPv4 => 4,
        ChType::UInt64
        | ChType::Int64
        | ChType::Float64
        | ChType::DateTime64(..)
        | ChType::Interval(_) => 8,
        ChType::Uuid | ChType::IPv6 => 16,
        ChType::Decimal { precision, .. } => decimal_width(*precision),
        ChType::FixedString(n) => *n as usize,
        ChType::String | ChType::Json(_) => 1,
        ChType::Nothing => {
            out.push(b'0');
            return Ok(());
        }
        other => {
            return Err(ChError::unsupported(format!(
                "A default value of {other} in a binary format"
            )));
        }
    };
    // A `String`'s default is its zero length.
    out.extend(std::iter::repeat_n(0, width));
    Ok(())
}

/// Appends row `row` of `array` (a column of `t`) in `RowBinary`'s encoding.
pub(crate) fn row_value(
    t: &ChType,
    array: &dyn Array,
    row: usize,
    settings: &FormatSettings,
    out: &mut Vec<u8>,
) -> Result<(), ChError> {
    let Some((array, row)) = resolve(array, row) else {
        // A `NULL` dictionary key: the type is `LowCardinality(Nullable(T))`.
        if t.is_nullable() {
            out.push(1);
            return Ok(());
        }
        return Err(ChError::logical_error(format!(
            "a NULL in a column of {t}, which is not Nullable"
        )));
    };
    match t {
        ChType::Nullable(inner) => {
            if array.is_null(row) {
                out.push(1);
                Ok(())
            } else {
                out.push(0);
                row_value(inner, array, row, settings, out)
            }
        }
        ChType::LowCardinality(inner) => row_value(inner, array, row, settings, out),
        ChType::SimpleAggregateFunction(spec) => {
            row_value(&spec.args[0], array, row, settings, out)
        }
        ChType::Array(inner) => {
            let list = array
                .as_list_opt::<i32>()
                .ok_or_else(|| bad_array(t, array))?;
            let offsets = list.value_offsets();
            let (start, end) = (offsets[row] as usize, offsets[row + 1] as usize);
            put_varuint(out, (end - start) as u64);
            let values = list.values();
            for i in start..end {
                row_value(inner, values.as_ref(), i, settings, out)?;
            }
            Ok(())
        }
        ChType::Map(k, v) => {
            let map = array.as_map_opt().ok_or_else(|| bad_array(t, array))?;
            let offsets = map.value_offsets();
            let (start, end) = (offsets[row] as usize, offsets[row + 1] as usize);
            put_varuint(out, (end - start) as u64);
            let (keys, values) = (map.keys(), map.values());
            for i in start..end {
                row_value(k, keys.as_ref(), i, settings, out)?;
                row_value(v, values.as_ref(), i, settings, out)?;
            }
            Ok(())
        }
        ChType::Tuple(elements) => {
            let st = array.as_struct_opt().ok_or_else(|| bad_array(t, array))?;
            for (i, (_, et)) in elements.iter().enumerate() {
                row_value(et, st.column(i).as_ref(), row, settings, out)?;
            }
            Ok(())
        }
        ChType::Json(_) if !settings.binary_write_json_as_string => {
            Err(ChError::unsupported(format!(
                "Columns of type {t} in RowBinary without \
                 output_format_binary_write_json_as_string = 1"
            )))
        }
        _ => scalar(t, array, row, out),
    }
}

/// The `RowBinary[WithNames[AndTypes]]` writer.
pub(crate) struct RowBinaryWriter<W: Write> {
    columns: Vec<Column>,
    settings: FormatSettings,
    out: W,
}

impl<W: Write> std::fmt::Debug for RowBinaryWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowBinaryWriter")
            .field("columns", &self.columns.len())
            .finish_non_exhaustive()
    }
}

impl<W: Write> RowBinaryWriter<W> {
    pub(crate) fn new(
        names: bool,
        types: bool,
        columns: Vec<Column>,
        settings: FormatSettings,
        mut out: W,
    ) -> Result<Self, ChError> {
        if names {
            let mut head = Vec::new();
            put_varuint(&mut head, columns.len() as u64);
            for c in &columns {
                put_string(&mut head, c.name.as_bytes());
            }
            if types {
                for c in &columns {
                    put_string(&mut head, c.ty.to_compact_string().as_bytes());
                }
            }
            out.write_all(&head).map_err(io_error)?;
        }
        Ok(Self {
            columns,
            settings,
            out,
        })
    }
}

impl<W: Write + Send> FormatWriter for RowBinaryWriter<W> {
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, batch)?;
        let mut out = Vec::new();
        for row in 0..batch.num_rows() {
            for (n, c) in self.columns.iter().enumerate() {
                row_value(
                    &c.ty,
                    batch.column(n).as_ref(),
                    row,
                    &self.settings,
                    &mut out,
                )?;
            }
        }
        self.out.write_all(&out).map_err(io_error)
    }

    fn write_totals(&mut self, _totals: &RecordBatch) -> Result<(), ChError> {
        Ok(())
    }

    fn write_extremes(&mut self, _extremes: &RecordBatch) -> Result<(), ChError> {
        Ok(())
    }

    fn finish(mut self: Box<Self>, _stats: &Stats) -> Result<(), ChError> {
        self.out.flush().map_err(io_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varuint_is_leb128() {
        let mut out = Vec::new();
        put_varuint(&mut out, 54492);
        assert_eq!(out, [0xdc, 0xa9, 0x03]);
        out.clear();
        put_varuint(&mut out, 0);
        assert_eq!(out, [0]);
    }

    #[test]
    fn ticks_convert_between_units() {
        // DateTime64(3) stored in milliseconds; DateTime64(2) in milliseconds.
        assert_eq!(ticks(1_500, TimeUnit::Millisecond, 3), 1_500);
        assert_eq!(ticks(1_500, TimeUnit::Millisecond, 2), 150);
        assert_eq!(ticks(-500, TimeUnit::Millisecond, 0), -1);
        assert_eq!(ticks(2, TimeUnit::Second, 3), 2_000);
    }
}
