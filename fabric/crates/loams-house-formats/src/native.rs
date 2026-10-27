//! The `Native` format: Arrow batches as ClickHouse's column blocks, framed
//! by `loams-house-native`'s codec.
//!
//! A column's data is its serialization prefix, then its values as
//! ClickHouse's `serializeBinaryBulk` writes them (`NativeWriter.cpp`,
//! `SerializationLowCardinality.cpp`, `SerializationNullable.cpp`,
//! `SerializationArray.cpp`, Apache-2.0); a column of no rows has no bytes at
//! all, prefix included:
//!
//! * scalars: each value's binary encoding ([`crate::binary`]), one after
//!   another;
//! * `Nullable(T)`: a null map (one byte per row, 1 for `NULL`), then `T`'s
//!   column with `T`'s default in each `NULL`'s place;
//! * `Array(T)`: cumulative `UInt64` offsets, then the elements' column;
//!   `Map(K, V)` is `Array(Tuple(K, V))`; `Tuple`: each element's column;
//! * `LowCardinality(T)`: prefix `UInt64` 1 (shared dictionaries with
//!   additional keys), then one granule: the index type (key width with the
//!   "additional keys" and "update dictionary" bits), the dictionary's size
//!   and values, the row count and one key per row. The dictionary is what a
//!   fresh `ColumnUnique` holds after inserting the rows: `NULL`'s
//!   placeholder (for `Nullable(T)`), `T`'s default, then the other values in
//!   order of first occurrence;
//! * `JSON` with `output_format_native_write_json_as_string`: prefix `UInt64`
//!   1, then a `String` column; any other `JSON` serialization is `48`.
//!
//! For a client of the native protocol before revision 54405,
//! `LowCardinality(T)` columns are sent as `T` (R9.41).

use std::collections::HashMap;
use std::io::Write;

use arrow_array::cast::AsArray;
use arrow_array::{Array, RecordBatch};
use loams_house_native::block::{Block, BlockInfo, Column as NativeColumn, Framing};
use loams_house_types::{ChError, ChType};

use crate::Column;
use crate::binary::{bad_array, default_scalar, is_null, resolve, scalar};
use crate::settings::FormatSettings;
use crate::writer::{FormatWriter, Stats, check_batch, io_error};

/// `LowCardinality`'s dictionary version: `SharedDictionariesWithAdditionalKeys`.
const LOW_CARDINALITY_VERSION: u64 = 1;
/// The index type's "the granule carries its own keys" bit.
const HAS_ADDITIONAL_KEYS: u64 = 1 << 9;
/// The index type's "the dictionary changed" bit.
const NEED_UPDATE_DICTIONARY: u64 = 1 << 10;
/// A `JSON` column's string serialization version.
const JSON_AS_STRING: u64 = 1;
/// The first revision whose clients read `LowCardinality`.
const LOW_CARDINALITY_REVISION: u64 = 54405;

/// Appends a column's serialization prefix.
fn prefix(t: &ChType, settings: &FormatSettings, out: &mut Vec<u8>) -> Result<(), ChError> {
    match t {
        ChType::LowCardinality(_) => out.extend_from_slice(&LOW_CARDINALITY_VERSION.to_le_bytes()),
        ChType::Json(_) => {
            if !settings.native_write_json_as_string {
                return Err(json_refused(t));
            }
            out.extend_from_slice(&JSON_AS_STRING.to_le_bytes());
        }
        ChType::Nullable(inner) | ChType::Array(inner) => prefix(inner, settings, out)?,
        ChType::Map(k, v) => {
            prefix(k, settings, out)?;
            prefix(v, settings, out)?;
        }
        ChType::Tuple(elements) => {
            for (_, et) in elements {
                prefix(et, settings, out)?;
            }
        }
        ChType::SimpleAggregateFunction(spec) => prefix(&spec.args[0], settings, out)?,
        _ => {}
    }
    Ok(())
}

fn json_refused(t: &ChType) -> ChError {
    ChError::unsupported(format!(
        "Columns of type {t} in Native without output_format_native_write_json_as_string = 1"
    ))
}

/// Appends the cumulative `UInt64` offsets of a list's rows and answers the
/// range of its values they cover.
fn offsets(raw: &[i32], out: &mut Vec<u8>) -> (usize, usize) {
    let first = raw.first().copied().unwrap_or(0);
    for o in &raw[1..] {
        out.extend_from_slice(&((o - first) as u64).to_le_bytes());
    }
    (
        first as usize,
        raw.last().copied().unwrap_or(first) as usize,
    )
}

/// Appends the data of every row of `array`, a column of `t` (no prefix).
fn data(
    t: &ChType,
    array: &dyn Array,
    settings: &FormatSettings,
    out: &mut Vec<u8>,
) -> Result<(), ChError> {
    let rows = array.len();
    if rows == 0 {
        return Ok(());
    }
    match t {
        ChType::Nullable(inner) => {
            for row in 0..rows {
                out.push(u8::from(is_null(array, row)));
            }
            for row in 0..rows {
                match resolve(array, row) {
                    Some((values, key)) if !values.is_null(key) => {
                        nested_scalar(inner, values, key, settings, out)?;
                    }
                    _ => default_scalar(inner.base(), out)?,
                }
            }
        }
        ChType::LowCardinality(inner) => low_cardinality(inner, array, out)?,
        ChType::SimpleAggregateFunction(spec) => data(&spec.args[0], array, settings, out)?,
        ChType::Array(inner) => {
            let list = array
                .as_list_opt::<i32>()
                .ok_or_else(|| bad_array(t, array))?;
            let (start, end) = offsets(list.value_offsets(), out);
            let values = list.values().slice(start, end - start);
            data(inner, values.as_ref(), settings, out)?;
        }
        ChType::Map(k, v) => {
            let map = array.as_map_opt().ok_or_else(|| bad_array(t, array))?;
            let (start, end) = offsets(map.value_offsets(), out);
            data(
                k,
                map.keys().slice(start, end - start).as_ref(),
                settings,
                out,
            )?;
            data(
                v,
                map.values().slice(start, end - start).as_ref(),
                settings,
                out,
            )?;
        }
        ChType::Tuple(elements) => {
            let st = array.as_struct_opt().ok_or_else(|| bad_array(t, array))?;
            if elements.is_empty() {
                out.extend(std::iter::repeat_n(0, rows));
            }
            for (i, (_, et)) in elements.iter().enumerate() {
                data(et, st.column(i).as_ref(), settings, out)?;
            }
        }
        ChType::Json(_) if !settings.native_write_json_as_string => return Err(json_refused(t)),
        _ => {
            for row in 0..rows {
                match resolve(array, row) {
                    Some((values, key)) if !values.is_null(key) => {
                        nested_scalar(t, values, key, settings, out)?;
                    }
                    _ => default_scalar(t, out)?,
                }
            }
        }
    }
    Ok(())
}

/// One scalar value, `t` possibly wrapped in `LowCardinality` or a
/// `SimpleAggregateFunction` (whose encodings are the inner type's).
fn nested_scalar(
    t: &ChType,
    array: &dyn Array,
    row: usize,
    settings: &FormatSettings,
    out: &mut Vec<u8>,
) -> Result<(), ChError> {
    match t {
        ChType::LowCardinality(inner) | ChType::Nullable(inner) => {
            nested_scalar(inner, array, row, settings, out)
        }
        ChType::SimpleAggregateFunction(spec) => {
            nested_scalar(&spec.args[0], array, row, settings, out)
        }
        ChType::Json(_) if !settings.native_write_json_as_string => Err(json_refused(t)),
        _ => scalar(t, array, row, out),
    }
}

/// A `LowCardinality(inner)` column's one granule (its version is in the
/// column's prefix).
fn low_cardinality(inner: &ChType, array: &dyn Array, out: &mut Vec<u8>) -> Result<(), ChError> {
    let rows = array.len();
    let (nullable, value_type) = match inner {
        ChType::Nullable(v) => (true, v.as_ref()),
        other => (false, other),
    };
    let mut dictionary: Vec<Vec<u8>> = Vec::new();
    if nullable {
        let mut placeholder = Vec::new();
        default_scalar(value_type, &mut placeholder)?;
        dictionary.push(placeholder);
    }
    let mut default = Vec::new();
    default_scalar(value_type, &mut default)?;
    let default_key = dictionary.len();
    dictionary.push(default.clone());
    let mut index: HashMap<Vec<u8>, usize> = HashMap::new();
    index.insert(default, default_key);
    let mut keys = Vec::with_capacity(rows);
    for row in 0..rows {
        let Some((values, key)) = resolve(array, row).filter(|(v, k)| !v.is_null(*k)) else {
            if !nullable {
                return Err(ChError::logical_error(format!(
                    "a NULL in a column of LowCardinality({inner})"
                )));
            }
            keys.push(0usize);
            continue;
        };
        let mut bytes = Vec::new();
        scalar(value_type, values, key, &mut bytes)?;
        let next = dictionary.len();
        let k = *index.entry(bytes.clone()).or_insert_with(|| {
            dictionary.push(bytes);
            next
        });
        keys.push(k);
    }
    let size = dictionary.len() as u64;
    let width_code: u64 = match size {
        0..=0x100 => 0,
        0x101..=0x1_0000 => 1,
        0x1_0001..=0x1_0000_0000 => 2,
        _ => 3,
    };
    out.extend_from_slice(
        &(width_code | HAS_ADDITIONAL_KEYS | NEED_UPDATE_DICTIONARY).to_le_bytes(),
    );
    out.extend_from_slice(&size.to_le_bytes());
    for entry in &dictionary {
        out.extend_from_slice(entry);
    }
    out.extend_from_slice(&(rows as u64).to_le_bytes());
    let width = 1usize << width_code;
    for k in keys {
        out.extend_from_slice(&(k as u64).to_le_bytes()[..width]);
    }
    Ok(())
}

/// One column's bytes in a block: prefix and data, nothing for no rows.
///
/// # Errors
///
/// A value the engine cannot encode, or a `JSON` column without
/// `output_format_native_write_json_as_string`.
pub fn column_data(
    t: &ChType,
    array: &dyn Array,
    settings: &FormatSettings,
) -> Result<Vec<u8>, ChError> {
    let mut out = Vec::new();
    if array.is_empty() {
        return Ok(out);
    }
    prefix(t, settings, &mut out)?;
    data(t, array, settings, &mut out)?;
    Ok(out)
}

/// A batch as a block: column names, type names and data. For a client
/// before revision 54405 `LowCardinality(T)` columns become `T`.
///
/// # Errors
///
/// As [`column_data`]; a `LowCardinality` nested in another type for an old
/// client is `48`.
pub fn encode_block(
    columns: &[Column],
    batch: &RecordBatch,
    settings: &FormatSettings,
) -> Result<Block, ChError> {
    check_batch(columns, batch)?;
    let mut block = Block {
        info: BlockInfo::default(),
        rows: batch.num_rows() as u64,
        columns: Vec::with_capacity(columns.len()),
    };
    for (n, c) in columns.iter().enumerate() {
        block.columns.push(NativeColumn {
            name: c.name.clone(),
            type_name: c.ty.to_compact_string(),
            data: column_data(&c.ty, batch.column(n).as_ref(), settings)?,
        });
    }
    if settings
        .client_revision
        .is_some_and(|r| r < LOW_CARDINALITY_REVISION)
    {
        block = loams_house_native::block::without_low_cardinality(block)
            .map_err(|e| ChError::unsupported(e.to_string()))?;
    }
    Ok(block)
}

/// The `Native` format writer: one block per non-empty batch, in the
/// `Native` format's framing (no `BlockInfo`, no serialization byte). A
/// result of no rows is its header block.
pub(crate) struct NativeWriter<W: Write> {
    columns: Vec<Column>,
    settings: FormatSettings,
    out: W,
    blocks: u64,
}

impl<W: Write> std::fmt::Debug for NativeWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeWriter")
            .field("blocks", &self.blocks)
            .finish_non_exhaustive()
    }
}

impl<W: Write> NativeWriter<W> {
    pub(crate) fn new(
        columns: Vec<Column>,
        settings: FormatSettings,
        out: W,
    ) -> Result<Self, ChError> {
        Ok(Self {
            columns,
            settings,
            out,
            blocks: 0,
        })
    }

    fn write_block(&mut self, block: &Block) -> Result<(), ChError> {
        let mut bytes = Vec::new();
        block.write(&mut bytes, Framing::NATIVE_FORMAT);
        self.out.write_all(&bytes).map_err(io_error)?;
        self.blocks += 1;
        Ok(())
    }
}

impl<W: Write + Send> FormatWriter for NativeWriter<W> {
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), ChError> {
        if batch.num_rows() == 0 {
            return check_batch(&self.columns, batch);
        }
        let block = encode_block(&self.columns, batch, &self.settings)?;
        self.write_block(&block)
    }

    fn write_totals(&mut self, _totals: &RecordBatch) -> Result<(), ChError> {
        Ok(())
    }

    fn write_extremes(&mut self, _extremes: &RecordBatch) -> Result<(), ChError> {
        Ok(())
    }

    fn finish(mut self: Box<Self>, _stats: &Stats) -> Result<(), ChError> {
        if self.blocks == 0 {
            let header = Block::header(
                self.columns
                    .iter()
                    .map(|c| (c.name.clone(), c.ty.to_compact_string())),
            );
            self.write_block(&header)?;
        }
        self.out.flush().map_err(io_error)
    }
}
