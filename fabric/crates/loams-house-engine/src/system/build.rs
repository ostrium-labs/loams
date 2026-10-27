//! A `system` table's rows as an Arrow batch in the engine's layout (plan HE1
//! Task 38), the way the front fills chDB's views (`catalog_select`): the
//! columns the House knows hold its values, and every other column its type's
//! default (`defaultValueOfTypeName`, [`loams_house_types::default`]).

use std::sync::Arc;

use datafusion::arrow::array::{
    ArrayRef, Int64Array, ListBuilder, RecordBatch, StringArray, StringBuilder, UInt32Array,
    UInt64Array,
};
use datafusion::arrow::compute::{CastOptions, cast_with_options, take};
use datafusion::arrow::datatypes::{DataType, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use loams_house_types::arrow::{data_type, to_arrow};
use loams_house_types::default::default_array;
use loams_house_types::{ChError, ChType};

/// One value of a `system` table's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    /// `NULL` (the column must be `Nullable`).
    Null,
    /// Text: `String`, and the `Date`/`DateTime` text ClickHouse reads.
    Str(String),
    /// An unsigned number: `UInt*`, `Bool`, an `Enum`'s value, or seconds
    /// since the epoch for a `DateTime`.
    UInt(u64),
    /// A signed number: `Int*`, `Enum` values, days for a `Date`.
    Int(i64),
    /// An `Array(String)`.
    Strs(Vec<String>),
}

impl From<&str> for Cell {
    fn from(s: &str) -> Self {
        Cell::Str(s.to_string())
    }
}

impl From<String> for Cell {
    fn from(s: String) -> Self {
        Cell::Str(s)
    }
}

impl From<u64> for Cell {
    fn from(v: u64) -> Self {
        Cell::UInt(v)
    }
}

impl From<bool> for Cell {
    fn from(v: bool) -> Self {
        Cell::UInt(u64::from(v))
    }
}

impl<T: Into<Cell>> From<Option<T>> for Cell {
    fn from(v: Option<T>) -> Self {
        v.map_or(Cell::Null, Into::into)
    }
}

/// What the House fills of a table: the columns it knows, and one row of
/// cells (in that order) per row. Every other column holds its default.
#[derive(Debug, Default, Clone)]
pub struct Rows {
    /// The filled columns.
    pub columns: Vec<&'static str>,
    /// The rows.
    pub rows: Vec<Vec<Cell>>,
}

impl Rows {
    /// Rows of `columns`.
    pub fn new(columns: &[&'static str]) -> Self {
        Self {
            columns: columns.to_vec(),
            rows: Vec::new(),
        }
    }

    /// Adds a row; its cells are in [`Rows::columns`]' order (a row of
    /// another width is refused when the batch is built).
    pub fn push(&mut self, row: Vec<Cell>) {
        self.rows.push(row);
    }
}

fn err(e: impl std::fmt::Display) -> ChError {
    ChError::logical_error(format!("Building a system table: {e}"))
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

/// `n` copies of `t`'s default value.
pub fn defaults(t: &ChType, n: usize) -> Result<ArrayRef, ChError> {
    let one = default_array(t)?;
    let indices = UInt32Array::from(vec![0u32; n]);
    take(one.as_ref(), &indices, None).map_err(err)
}

/// A column of type `t` holding `cells`.
///
/// # Errors
///
/// A cell that does not fit the type (a programming error).
pub fn column(t: &ChType, cells: &[Cell]) -> Result<ArrayRef, ChError> {
    let (target, _) = data_type(t)?;
    let base = t.strip_nullable();
    match base {
        ChType::String => {
            let values: StringArray = cells
                .iter()
                .map(|c| match c {
                    Cell::Null => Ok(None),
                    Cell::Str(s) => Ok(Some(s.clone())),
                    Cell::UInt(v) => Ok(Some(v.to_string())),
                    Cell::Int(v) => Ok(Some(v.to_string())),
                    Cell::Strs(_) => Err(err(format!("an array for {t}"))),
                })
                .collect::<Result<_, _>>()?;
            cast(&(Arc::new(values) as ArrayRef), &target)
        }
        ChType::UInt8
        | ChType::UInt16
        | ChType::UInt32
        | ChType::UInt64
        | ChType::Bool
        | ChType::DateTime(_) => {
            let values: UInt64Array = cells
                .iter()
                .map(|c| match c {
                    Cell::Null => Ok(None),
                    Cell::UInt(v) => Ok(Some(*v)),
                    Cell::Int(v) => u64::try_from(*v)
                        .map(Some)
                        .map_err(|_| err(format!("{v} for {t}"))),
                    other => Err(err(format!("{other:?} for {t}"))),
                })
                .collect::<Result<_, _>>()?;
            let values: ArrayRef = Arc::new(values);
            match base {
                // Seconds since the epoch.
                ChType::DateTime(_) => cast(&cast(&values, &DataType::Int64)?, &target),
                _ => cast(&values, &target),
            }
        }
        ChType::Int8
        | ChType::Int16
        | ChType::Int32
        | ChType::Int64
        | ChType::Enum8(_)
        | ChType::Enum16(_)
        | ChType::Date => {
            let values: Int64Array = cells
                .iter()
                .map(|c| match c {
                    Cell::Null => Ok(None),
                    Cell::Int(v) => Ok(Some(*v)),
                    Cell::UInt(v) => i64::try_from(*v)
                        .map(Some)
                        .map_err(|_| err(format!("{v} for {t}"))),
                    other => Err(err(format!("{other:?} for {t}"))),
                })
                .collect::<Result<_, _>>()?;
            let values: ArrayRef = Arc::new(values);
            match base {
                // Days since the epoch.
                ChType::Date => cast(&cast(&values, &DataType::Int32)?, &target),
                _ => cast(&values, &target),
            }
        }
        ChType::Array(inner) if matches!(inner.as_ref(), ChType::String) => {
            let DataType::List(field) = &target else {
                return Err(err("Array is not a List"));
            };
            let mut builder = ListBuilder::new(StringBuilder::new()).with_field(field.clone());
            for c in cells {
                match c {
                    Cell::Strs(items) => {
                        for i in items {
                            builder.values().append_value(i);
                        }
                        builder.append(true);
                    }
                    other => return Err(err(format!("{other:?} for {t}"))),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        _ => Err(err(format!("values of {t} (only its default is filled)"))),
    }
}

/// The schema of `shape`, each field with its ClickHouse type.
pub fn schema(shape: &[(&str, &str)]) -> Result<SchemaRef, ChError> {
    let fields = shape
        .iter()
        .map(|(name, ty)| to_arrow(name, &loams_house_types::parse(ty)?))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Arc::new(Schema::new(fields)))
}

/// One batch of `shape`'s columns: those in `rows` filled, the rest at their
/// defaults.
///
/// # Errors
///
/// A filled column that is not in `shape`, a row of the wrong width, or a
/// cell that does not fit its column's type.
pub fn batch(shape: &[(&str, &str)], rows: &Rows) -> Result<RecordBatch, ChError> {
    for c in &rows.columns {
        if !shape.iter().any(|(n, _)| n == c) {
            return Err(err(format!("{c} is not a column of the table")));
        }
    }
    if let Some(bad) = rows.rows.iter().find(|r| r.len() != rows.columns.len()) {
        return Err(err(format!(
            "a row of {} cells for {} columns",
            bad.len(),
            rows.columns.len()
        )));
    }
    let schema = schema(shape)?;
    let n = rows.rows.len();
    let mut arrays = Vec::with_capacity(shape.len());
    for (name, ty) in shape {
        let t = loams_house_types::parse(ty)?;
        let array = match rows.columns.iter().position(|c| c == name) {
            Some(i) => {
                let cells: Vec<Cell> = rows.rows.iter().map(|r| r[i].clone()).collect();
                column(&t, &cells).map_err(|e| err(format!("column {name}: {}", e.message)))?
            }
            None => defaults(&t, n)?,
        };
        arrays.push(array);
    }
    RecordBatch::try_new(schema, arrays).map_err(err)
}

/// [`batch`] as an in-memory table.
///
/// # Errors
///
/// As [`batch`].
pub fn table(shape: &[(&str, &str)], rows: &Rows) -> Result<Arc<MemTable>, ChError> {
    let batch = batch(shape, rows)?;
    let table = MemTable::try_new(batch.schema(), vec![vec![batch]]).map_err(err)?;
    Ok(Arc::new(table))
}
